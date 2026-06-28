//! PRINT mode — one-shot human-oriented text output (func-11 R-11-005/009; arch-11 §2.2).
//!
//! Drives a single prompt to completion over the [`AgentSession`] seam, then writes the final
//! assistant text to the caller-supplied [`std::io::Write`] sink. Optionally narrates tool activity
//! as it streams. The sink and the input are parameters — the binary wires real stdout/stdin later.

use std::io::Write;

use cyrup_session_svc::{AgentSession, AgentSessionEvent, UserInput};
use futures::StreamExt;

use crate::error::ModesError;

/// Knobs for [`run_print`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PrintOptions {
    /// Narrate `[tool] <name>` lines as tools begin executing (human debugging aid).
    pub show_tools: bool,
}

/// Run `input` to completion and write the final assistant text to `out` (R-11-005).
///
/// Drains the run event stream (which terminates after `agent_end`), so the run progresses to
/// completion; with [`PrintOptions::show_tools`] each tool start is narrated. The final assistant
/// text on the branch is then written as a single line. Returns once the run has fully settled.
pub async fn run_print<W: Write>(
    session: &AgentSession,
    input: impl Into<UserInput>,
    out: &mut W,
    opts: PrintOptions,
) -> Result<(), ModesError> {
    let mut stream = session.prompt(input).await?;
    while let Some(ev) = stream.next().await {
        if opts.show_tools
            && let AgentSessionEvent::ToolExecutionStart { tool_name, .. } = &ev
        {
            writeln!(out, "[tool] {tool_name}")?;
        }
    }
    // The run stream ends at `agent_end`; this is a cheap confirmation the agent is idle.
    session.wait_for_idle().await;

    if let Some(text) = session.last_assistant_text().await {
        writeln!(out, "{text}")?;
    }
    out.flush()?;
    Ok(())
}
