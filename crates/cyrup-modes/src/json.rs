//! JSON mode — JSONL event stream (func-11 R-11-007; arch-11 §2.2).
//!
//! Runs the same one-shot processing as PRINT mode, but instead of the final text it serializes
//! every [`AgentSessionEvent`] as one JSON object per line to the caller-supplied sink — the stable
//! machine-readable event stream external tools parse. One schema serves json and rpc: both write
//! the [`crate::JsonAgentSessionEvent`] projection, never the raw event (Pi `toJsonEvent`,
//! json-event.ts:28; applied at print-mode.ts:110 for json mode and rpc-mode.ts:356 for rpc).
//!
//! # Backpressure
//!
//! Pi pairs the v0.84.1 projection with `waitForRawStdoutBackpressure` (print-mode.ts:11,113-118),
//! subscribed on the AGENT. That exists because Pi's `writeRawStdout` is fire-and-forget — it chains
//! onto a promise tail and does not await it (`output-guard.ts:85-93`), so a slow stdout would
//! otherwise buffer without bound; `waitForRawStdoutBackpressure` (`:95-103`) drains that tail.
//!
//! cyrup needs no analog and one is deliberately NOT added: the loop below writes through a blocking
//! [`std::io::Write`] and flushes each line, so the write itself is the await, and the event stream
//! feeding it is the seam's awaited bounded-1024 fan-out (`cyrup-session-svc/src/subscriber.rs:23`,
//! `:63-73` — "awaited (backpressure → slows the agent, never drops)"). A stalled consumer therefore
//! already stalls the agent, which is exactly the invariant Pi's agent subscription restores. Adding
//! a drain here would be a no-op over an already-drained sink.

use std::io::Write;
use std::sync::Arc;

use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, BindOptions, EventStream, PromptAccepted,
    UserInput,
};
use futures::{FutureExt, StreamExt};

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
    let mut session = runtime.session().await;
    if session.claim_json_header() {
        let header = session.session_header().await;
        let line = serde_json::to_string(&header)?;
        // TOOL-037 — pi's header goes out through `writeRawStdout` like every other protocol line
        // (`print-mode.ts:112-118` @v0.84.1), so it carries the same EAGAIN/EWOULDBLOCK/ENOBUFS
        // retry (`core/output-guard.ts:36-41`). A plain `writeln!` turned a non-blocking stdout —
        // the shape a supervisor or CI pipe presents — into a mode-fatal `io::Error`.
        crate::raw_stdout::write_raw_stdout(out, &format!("{line}\n")).await?;
        crate::raw_stdout::flush_raw_stdout(out).await?;
    }

    // Pi's `rebindSession()` (print-mode.ts:71-119): bind the extension host — with the `onError`
    // sink, SEAM-006 — and then install ONE session-wide subscription:
    //
    //   unsubscribe?.();
    //   unsubscribe = session.subscribe((event) => { if (mode === "json") writeRawStdout(...); });
    //
    // SEAM-027 — that subscription is session-scoped and is held ACROSS the whole message loop
    // (`:129-137`), torn down only in `disposeRuntime()`. cyrup used to take the RUN-scoped stream
    // `AgentSession::prompt` returns and drain only that, so every event emitted BETWEEN runs —
    // extension UI, `session_info_changed`, `model_changed`, background compaction progress — was
    // silently absent from the json stream, and a consumer with `--follow-up` saw an incomplete
    // event log.
    let mut events = bind_and_subscribe(&session).await;

    for input in messages {
        // Pi's `rebindSession` (print-mode.ts:71-72): re-read the runtime's active session so a
        // message submitted after a replacement addresses the NEW session — and, exactly as pi does
        // at `:106-108`, drop the old subscription and take a fresh one, because the previous
        // stream was terminated with `SessionReplaced` (R-11-021). Modelled on the RPC host's
        // `rebind_session` (`rpc.rs`), which is pi's other consumer of the same callback.
        let active = runtime.session().await;
        if !Arc::ptr_eq(&session, &active) {
            session = active;
            events = bind_and_subscribe(&session).await;
        }

        // The run is observed through the PERSISTENT subscription, so the submission only has to
        // resolve to pi's `preflightResult` — `prompt_accepted` is the seam documented for exactly
        // "adapters that manage their own persistent subscription".
        let accepted = session.prompt_accepted(input).await?;
        if matches!(accepted, PromptAccepted::Handled) {
            // An `input` extension handler serviced the submission and no run started, so no
            // `agent_settled` will arrive; pi's `await session.prompt(...)` likewise resolves at
            // once. Fall through to the next message.
            continue;
        }
        // Drain until this run settles. `agent_settled` is pi's own terminal for a submission
        // (`AgentSettledEvent`, extensions/types.ts:721-725) and is what the RPC host waits on too.
        while let Some(ev) = events.next().await {
            let settled = matches!(ev, AgentSessionEvent::AgentSettled);
            write_event(out, &ev).await?;
            if settled {
                break;
            }
        }
    }

    // Everything the session emitted after the last settle and before the mode returns — pi's
    // subscription is still installed at this point too (it is removed by `disposeRuntime()`,
    // print-mode.ts:152-157, which runs after `runPrintMode` returns). Non-blocking: take only what
    // is already queued, so a mode with no further events does not wait for one.
    while let Some(Some(ev)) = events.next().now_or_never() {
        write_event(out, &ev).await?;
    }
    Ok(())
}

/// Pi's `rebindSession` body (print-mode.ts:71-112): `bindExtensions({..., onError})` then a fresh
/// session-wide `session.subscribe(...)`. Returns the new stream; the caller drops the old one,
/// which is cyrup's `unsubscribe?.()` (`:106`).
async fn bind_and_subscribe(session: &Arc<AgentSession>) -> EventStream<AgentSessionEvent> {
    session
        .bind_extensions_with(BindOptions {
            on_error: Some(crate::print::extension_error_sink()),
        })
        .await;
    session.subscribe()
}

/// Pi print-mode.ts:110 — `writeRawStdout(`${JSON.stringify(toJsonEvent(event))}\n`)`. The
/// projection (never the raw event) is what goes on the wire; see [`crate::to_json_event`] for what
/// it drops and why.
async fn write_event<W: Write>(out: &mut W, ev: &AgentSessionEvent) -> Result<(), ModesError> {
    // SEAM-080 / SEAM-081. pi's json mode is `session.subscribe(event => writeRawStdout(...))`
    // (`print-mode.ts:74` @v0.83.0), so its line set is exactly the `AgentSessionEvent` union.
    // cyrup's enum is a super-set, and the four cyrup-only members (`session_replaced`,
    // `model_changed`, `session_start`, `session_shutdown`) must not reach the stream. The rpc host
    // filtered `session_replaced` at both of its write sites; this mode had NO guard at all, so all
    // four went out. See [`crate::is_upstream_wire_event`].
    if !crate::is_upstream_wire_event(ev) {
        return Ok(());
    }
    let line = serde_json::to_string(&crate::to_json_event(ev))?;
    // TOOL-037 — pi's `writeRawStdout` (`core/output-guard.ts:85` → `writeRawStdoutChunk`,
    // `:20-43`), retry loop included. See `crate::raw_stdout`.
    crate::raw_stdout::write_raw_stdout(out, &format!("{line}\n")).await?;
    crate::raw_stdout::flush_raw_stdout(out).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::write_event;
    use cyrup_session_svc::AgentSessionEvent;

    /// Drive [`write_event`] directly — it is the single serializer both the per-run drain and the
    /// trailing non-blocking drain go through — and collect the produced bytes.
    fn wire(ev: &AgentSessionEvent) -> String {
        let mut buf: Vec<u8> = Vec::new();
        // `write_event` awaits only `write_raw_stdout`/`flush_raw_stdout`, which never yield for a
        // `Vec<u8>` sink, so the future is ready on the first poll and a current-thread runtime is
        // the lightest correct driver.
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread runtime")
            .block_on(async { write_event(&mut buf, ev).await })
            .expect("writing to a Vec cannot fail");
        String::from_utf8(buf).expect("json is utf-8")
    }

    /// SEAM-080 + SEAM-081 — RED before this pass.
    ///
    /// pi's json mode is `session.subscribe(event => writeRawStdout(JSON.stringify(...)))`
    /// (`modes/print-mode.ts:74` @v0.83.0) and its listener type is `AgentSessionEventListener`, so
    /// the stdout line set is EXACTLY pi's `AgentSessionEvent` union (`core/agent-session.ts:139-181`
    /// @v0.83.0). cyrup's enum is a super-set with four extra members and `write_event` had NO guard
    /// whatsoever, so before this pass every one of these four produced a full JSON line:
    /// `{"type":"model_changed",…}`, `{"type":"session_start",…}`, `{"type":"session_shutdown",…}`
    /// and `{"type":"session_replaced",…}`. Each assertion below fails on the pre-fix code.
    #[test]
    fn cyrup_only_events_never_reach_the_json_stdout_stream() {
        let invented = [
            AgentSessionEvent::ModelChanged {
                provider: "anthropic".into(),
                model: "claude".into(),
            },
            AgentSessionEvent::SessionStart {
                reason: "startup".into(),
                previous_session_file: None,
            },
            AgentSessionEvent::SessionShutdown {
                reason: "quit".into(),
            },
            AgentSessionEvent::SessionReplaced { generation: 2 },
        ];
        for ev in &invented {
            assert_eq!(
                wire(ev),
                "",
                "`{}` is not a member of pi's AgentSessionEvent union and must not reach stdout",
                ev.kind()
            );
        }
    }

    /// Presence before absence: the filter must not have taken a genuine upstream event with it.
    /// `thinking_level_changed` and `session_info_changed` are the two members that sit right beside
    /// the removed ones in cyrup's enum and ARE in pi's union (`agent-session.ts:153-154`).
    #[test]
    fn genuine_upstream_events_still_reach_the_json_stdout_stream() {
        let kept = [
            AgentSessionEvent::ThinkingLevelChanged {
                level: "high".into(),
            },
            AgentSessionEvent::SessionInfoChanged {
                name: Some("work".into()),
            },
            AgentSessionEvent::AgentSettled,
        ];
        for ev in &kept {
            let line = wire(ev);
            assert!(
                line.contains(&format!("\"type\":\"{}\"", ev.kind())),
                "`{}` is in pi's union and must still be written; got {line:?}",
                ev.kind()
            );
            assert!(line.ends_with('\n'), "every protocol line is LF-terminated");
        }
    }
}
