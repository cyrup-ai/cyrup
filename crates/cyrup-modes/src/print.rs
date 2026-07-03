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
use cyrup_session_svc::{AgentSession, AgentSessionEvent, UserInput};
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
pub async fn run_print<W, E>(
    session: &AgentSession,
    messages: impl IntoIterator<Item = UserInput>,
    out: &mut W,
    err: &mut E,
    opts: PrintOptions,
) -> Result<(), ModesError>
where
    W: Write,
    E: Write,
{
    // Send loop (Pi print-mode.ts:121-127): prompt each message to completion, in order, producing
    // no assistant output. Each run stream terminates at `agent_end`; `wait_for_idle` then confirms
    // the agent is settled before the next prompt is submitted.
    for input in messages {
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
    // outside the loop. Only an assistant final message produces output.
    let transcript = session.messages().await;
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
                writeln!(err, "{message}")?;
            }
            // A clean turn: one line per text content block (Pi print-mode.ts:138-144).
            _ => {
                for content in &assistant.content {
                    if let Content::Text { text, .. } = content {
                        writeln!(out, "{text}")?;
                    }
                }
            }
        }
    }
    out.flush()?;
    err.flush()?;
    Ok(())
}
