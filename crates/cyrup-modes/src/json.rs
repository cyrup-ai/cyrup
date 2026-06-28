//! JSON mode — JSONL event stream (func-11 R-11-007; arch-11 §2.2).
//!
//! Runs the same one-shot processing as PRINT mode, but instead of the final text it serializes
//! every [`AgentSessionEvent`] as one JSON object per line to the caller-supplied sink — the stable
//! machine-readable event stream external tools parse. One schema serves json and rpc.

use std::io::Write;

use cyrup_session_svc::{AgentSession, UserInput};
use futures::StreamExt;

use crate::error::ModesError;

/// Run `input` to completion, emitting each [`AgentSessionEvent`] as one JSONL record to `out`.
///
/// The run event stream terminates after `agent_end`, so this returns once the run is complete.
/// Each line is a single `serde_json` object (`{"type":"agent_start"}`, …); consumers split on `\n`.
pub async fn run_json<W: Write>(
    session: &AgentSession,
    input: impl Into<UserInput>,
    out: &mut W,
) -> Result<(), ModesError> {
    let mut stream = session.prompt(input).await?;
    while let Some(ev) = stream.next().await {
        let line = serde_json::to_string(&ev)?;
        writeln!(out, "{line}")?;
        out.flush()?;
    }
    Ok(())
}
