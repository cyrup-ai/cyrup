//! Non-interactive mode dispatch over injectable readers/writers (arch-11 §2.4/§5; R-11-005/007/011).
//!
//! Thin glue over [`cyrup_modes`]: PRINT/JSON take a [`std::io::Write`] sink, RPC takes an async
//! reader + writer. The sinks are parameters so tests drive `Vec<u8>` buffers and the binary wires
//! real stdio. PRINT/JSON replay follow-up messages one prompt at a time after the initial run
//! (R-11-009).

use std::io::Write;

use cyrup_modes::{PrintOptions, run_json, run_print, run_rpc};
use cyrup_session_svc::{AgentSession, AgentSessionRuntime, InputSource, UserInput};
use tokio::io::{AsyncBufRead, AsyncWrite};

use crate::input::Inputs;

/// PRINT dispatch: prompt the initial submission then each follow-up in order as ONE ordered turn,
/// writing only the *final* transcript message to `out` (Pi print-mode.ts:129-146); a failed/aborted
/// final turn routes its error to stderr with no assistant stdout. Returns the process exit code
/// derived from the terminal stop reason (R-11-005, arch-11 §6.6).
pub async fn run_print_dispatch<W: Write>(
    runtime: &AgentSessionRuntime,
    inputs: &Inputs,
    out: &mut W,
) -> anyhow::Result<i32> {
    // The initial session is bound + announced by `run_print` itself, at Pi's point — the
    // `rebindSession()` → `session.bindExtensions()` at print-mode.ts:119 → :73, ahead of the send
    // loop at :121. `main.rs` therefore builds this runtime with
    // `AgentSessionRuntime::create_unannounced` and applies `--name`/`--models` in between
    // (SEAM-033); see [`announce_session_start`].
    let messages =
        std::iter::once(initial_input(inputs)).chain(inputs.follow_ups.iter().map(|f| cli_input(f)));
    let mut err = std::io::stderr();
    let ran = run_print(runtime, messages, out, &mut err, PrintOptions::default()).await;
    let code = exit_code(&*runtime.session().await).await;
    // Teardown on EVERY exit path — Pi's `finally { await disposeRuntime() }` (print-mode.ts:152-157),
    // which emits `session_shutdown{reason:"quit"}` before releasing the session (see
    // [`dispose_session`]). The exit code is read FIRST because dispose aborts the run.
    runtime.dispose().await;
    ran?;
    Ok(code)
}

/// JSON dispatch: run the initial prompt then each follow-up, streaming every event as JSONL to `out`.
///
/// JSON mode ALWAYS returns exit 0 (Pi `print-mode.ts:34,129-148`): `exitCode` inits to `0` and is
/// mutated only inside the `if (mode === "text")` branch, so a failed/aborted final turn NEVER changes
/// the JSON-mode exit code — a consumer scripting `cyrup --mode json … ; echo $?` relies on the
/// always-0 convention. (The terminal stop reason is still observable in the streamed event records.)
pub async fn run_json_dispatch<W: Write>(
    runtime: &AgentSessionRuntime,
    inputs: &Inputs,
    out: &mut W,
) -> anyhow::Result<i32> {
    // Same startup bind as PRINT, done by `run_json` itself — but AFTER the JSONL header, which is
    // exactly pi's order (header at print-mode.ts:112-118, `await rebindSession()` at :119).
    let messages =
        std::iter::once(initial_input(inputs)).chain(inputs.follow_ups.iter().map(|f| cli_input(f)));
    let ran = run_json(runtime, messages, out).await;
    // Same `finally { await disposeRuntime() }` as PRINT (print-mode.ts:152-157 serves both modes).
    runtime.dispose().await;
    ran?;
    Ok(0)
}

/// Bind the extension host to `session` and announce it with `session_start{reason:"startup"}` (Pi
/// `session.bindExtensions(...)`, print-mode.ts:73 → agent-session.ts:2250, whose event defaults to
/// `{type:"session_start", reason:"startup"}` at agent-session.ts:389).
///
/// The mirror image of [`dispose_session`]. Every first-party host now takes an
/// [`AgentSessionRuntime`] (SEAM-006 moved print/json onto the runtime, matching
/// `runPrintMode(runtimeHost, …)`, print-mode.ts:32) and gets its announcement either from
/// `AgentSessionRuntime::create` (interactive/RPC) or from the mode entry point itself
/// (print/json — SEAM-033, so `--name`/`--models` are applied first), so this remains for
/// EMBEDDERS that drive a bare [`AgentSession`]. Without it no extension
/// observes the session, so the permission gate never refreshes its per-cwd policy or starts the
/// ask-forwarding watcher, subagents never reset background-run tracking, and intercom's
/// `SessionStart` arm never runs.
///
/// Idempotent per session (`AgentSession::bind_extensions`), so a host that also drives a runtime
/// cannot announce twice.
pub async fn announce_session_start(session: &AgentSession) {
    session.bind_extensions().await;
}

/// Emit `session_shutdown{reason:"quit"}` and tear the session down (Pi `AgentSessionRuntime.dispose`
/// → `session.dispose()`, agent-session-runtime.ts:397-404).
///
/// The bare-[`AgentSession`] teardown for embedders; the first-party hosts reach the same code
/// through `AgentSessionRuntime::dispose()`. Without it no extension ever observes
/// `session_shutdown` on a normal exit, so anything that flushes or deregisters on shutdown
/// (intercom broker deregistration, subagent background-run cleanup, permission-store teardown) never
/// runs, and an in-flight run is never settled before the process returns.
pub async fn dispose_session(session: &AgentSession) {
    session.dispose("quit").await;
}

/// RPC dispatch: serve the persistent stdio line protocol over `reader`/`writer` (R-11-011…016).
/// Drives the [`AgentSessionRuntime`] host so the session-replacing commands
/// (`new_session`/`switch_session`/`fork`/`clone`) rebuild the active session and rebind (Pi
/// `rpc-mode.ts` `runtimeHost`).
pub async fn run_rpc_dispatch<R, W>(
    runtime: &AgentSessionRuntime,
    reader: R,
    writer: &mut W,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let ran = run_rpc(runtime, reader, writer).await;
    // Reader EOF (Pi's `process.stdin.on("end", …) → shutdown()`, rpc-mode.ts:801-803) tears the
    // runtime down: `session_shutdown{reason:"quit"}` then `session.dispose()` (rpc-mode.ts:723-739).
    runtime.dispose().await;
    ran?;
    Ok(())
}

/// The process exit code for a settled one-shot run, from the last assistant message's stop reason
/// (arch-11 §6.6): `error` ⇒ 1, `aborted` ⇒ 130, otherwise (`stop`/`length`/`toolUse`) ⇒ 0.
pub async fn exit_code(session: &AgentSession) -> i32 {
    use cyrup_sdk::core::{Message, StopReason};
    for message in session.messages().await.iter().rev() {
        if let Message::Assistant(assistant) = message {
            return match assistant.stop_reason {
                StopReason::Error => 1,
                StopReason::Aborted => 130,
                _ => 0,
            };
        }
    }
    0
}

/// Wrap one-shot text as a CLI-sourced [`UserInput`].
fn cli_input(text: &str) -> UserInput {
    UserInput::text(text.to_string(), InputSource::Cli)
}

/// The initial submission: the assembled text plus any image `@file` attachments (Pi
/// `initialImages`, initial-message.ts:41).
pub fn initial_input(inputs: &Inputs) -> UserInput {
    let mut input = UserInput::text(inputs.initial.clone(), InputSource::Cli);
    input.images = inputs.images.clone();
    input
}
