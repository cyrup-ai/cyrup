//! JSON mode — JSONL event stream (func-11 R-11-007; arch-11 §2.2).
//!
//! Runs the same one-shot processing as PRINT mode, but instead of the final text it serializes
//! every [`AgentSessionEvent`] as one JSON object per line to the caller-supplied sink — the stable
//! machine-readable event stream external tools parse. One schema serves json and rpc.

use std::io::Write;

use cyrup_session_svc::{AgentSessionRuntime, UserInput};
use futures::StreamExt;

use crate::error::ModesError;

/// Run `messages` to completion in order, emitting each [`cyrup_session_svc::AgentSessionEvent`] as
/// one JSONL record to `out`.
///
/// Before the first event, the session header (`sessionManager.getHeader()` →
/// `{"type":"session",…}`) is written as JSONL line 1, matching Pi's `runPrintMode`, which writes
/// `JSON.stringify(header)` ahead of the event subscription (print-mode.ts:112-117). The header is
/// emitted at most once per session ([`cyrup_session_svc::AgentSession::claim_json_header`]) — Pi
/// writes it a single time before its whole message loop, so a second `run_json` call on the same
/// session does not repeat it.
///
/// Each run's event stream terminates after `agent_end`, so this returns once every message has run.
/// Each line is a single `serde_json` object (`{"type":"agent_start"}`, …); consumers split on `\n`.
///
/// Takes the RUNTIME rather than a bare session for the same reason [`crate::run_print`] does
/// (SEAM-006): Pi's json mode IS `runPrintMode(runtimeHost, {mode:"json"})` (print-mode.ts:32,:73),
/// so a loaded extension's control ops have a host to act on and the send loop re-reads the active
/// session between messages.
pub async fn run_json<W: Write>(
    runtime: &AgentSessionRuntime,
    messages: impl IntoIterator<Item = UserInput>,
    out: &mut W,
) -> Result<(), ModesError> {
    // Pi writes the header ONCE, from the session that is active when the mode starts, before
    // `rebindSession()` and the send loop (print-mode.ts:112-119).
    let session = runtime.session().await;
    if session.claim_json_header() {
        let header = session.session_header().await;
        let line = serde_json::to_string(&header)?;
        writeln!(out, "{line}")?;
        out.flush()?;
    }
    // Pi orders the mode's `try` block header-then-bind: `writeRawStdout(header)` at
    // print-mode.ts:112-118, then `await rebindSession()` at :119, then the send loop at :121.
    // `rebindSession` ends in `session.bindExtensions(...)` (:73) whose tail emits
    // `_sessionStartEvent` (agent-session.ts:2250). SEAM-033: the announcement belongs HERE, not in
    // the runtime constructor, because `main.ts` applies `--name` (:650) and `--models` (:742-750)
    // in between. Idempotent per session, so a host that announced at construction is unaffected.
    session.bind_extensions().await;
    drop(session);

    for input in messages {
        // Pi's `rebindSession` (print-mode.ts:71-72): re-read the runtime's active session so a
        // message submitted after a replacement addresses the NEW session.
        let session = runtime.session().await;
        let mut stream = session.prompt(input).await?;
        while let Some(ev) = stream.next().await {
            let line = serde_json::to_string(&ev)?;
            writeln!(out, "{line}")?;
            out.flush()?;
        }
    }
    Ok(())
}
