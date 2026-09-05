//! Non-interactive mode dispatch over injectable readers/writers (arch-11 §2.4/§5; R-11-005/007/011).
//!
//! Thin glue over [`cyrup_modes`]: PRINT/JSON take a [`std::io::Write`] sink, RPC takes an async
//! reader + writer. The sinks are parameters so tests drive `Vec<u8>` buffers and the binary wires
//! real stdio. PRINT/JSON replay follow-up messages one prompt at a time after the initial run
//! (R-11-009).

use std::io::Write;
use std::sync::Arc;

use cyrup_modes::{PrintOptions, run_json, run_print, run_rpc};
use cyrup_session_svc::{AgentSession, AgentSessionRuntime, InputSource, UserInput};
use tokio::io::{AsyncBufRead, AsyncWrite};

use crate::input::Inputs;

/// PRINT dispatch: prompt the initial submission then each follow-up in order as ONE ordered turn,
/// writing only the *final* transcript message to `out` (Pi print-mode.ts:129-146); a failed/aborted
/// final turn routes its error to stderr with no assistant stdout. Returns the process exit code
/// derived from the terminal stop reason (R-11-005, arch-11 §6.6).
///
/// A prompt-less run is legal and submits nothing at all — see [`turn_inputs`].
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
    let messages = turn_inputs(inputs);
    let mut err = std::io::stderr();
    // SEAM-016: the code comes from `run_print` itself, which decides it inside pi's terminal
    // output block from the same `lastMessage` it prints (print-mode.ts:139-148) and RETURNS it
    // (`return exitCode;`, :151) — exactly as `runPrintMode` does. It used to be recomputed here by
    // reverse-scanning the transcript, which disagreed with the printed message whenever the final
    // transcript entry was not an assistant message, and used a 130/1 mapping pi never emits.
    let ran = run_print(runtime, messages, out, &mut err, PrintOptions::default()).await;
    // Teardown on EVERY exit path — Pi's `finally { await disposeRuntime() }` (print-mode.ts:152-157),
    // which emits `session_shutdown{reason:"quit"}` before releasing the session (see
    // [`dispose_session`]).
    runtime.dispose().await;
    // Pi's `catch (error) { console.error(...); return 1; }` (print-mode.ts:153-155) sits INSIDE the
    // same try, so a mode error is exit 1 — which is what the `?` produces through `main`.
    Ok(ran?)
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
    let messages = turn_inputs(inputs);
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
///
/// A read failure on `reader` (a broken pipe, an `EIO` on a serial/socket fd, a supervisor tearing
/// the input fd down) propagates out of [`run_rpc`] and therefore out of here, so a command stream
/// severed mid-protocol exits non-zero instead of being indistinguishable from a client that closed
/// stdin cleanly. The runtime is disposed either way, before the error is returned.
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

/// ACP dispatch: serve the Agent Client Protocol on stdio until the client closes the connection
/// (`ACP-003`, `ACP-004`, `ACP-005`). Modelled on [`run_rpc_dispatch`], with **three** structural
/// divergences from its sibling, each of which is a unit.
///
/// 1. **It takes no [`AgentSessionRuntime`].** `run_rpc` opens with
///    `runtime.session().await.bind_extensions().await` (SEAM-033 — the host announces after
///    `--name`/`--models`), but ACP must announce after `initialize` settles, because `has_ui` and
///    the client's advertised capabilities are what a `session_start` handler should see. So the
///    runtime is built **lazily on `session/new`**, through `host`, and the teardown that
///    `run_rpc_dispatch` performs unconditionally is performed by
///    `cyrup_acp::SessionManager`'s own slot instead (`ACP-003`, `ACP-023`).
/// 2. **A broken pipe is a clean exit, not an error** (`ACP-004`). `Stdio::connect_to` is
///    `blocking::Unblock` over `std::io::stdout()` feeding a sink around `write_line`,
///    `transport_outgoing_lines_actor` surfaces the `io::Error` verbatim, and the ACP crate handles
///    no `BrokenPipe` itself — so a broken pipe propagates out of `connect_to`.
///
///    **`CYRUP-DELTA`:** cyrup's RPC sibling does the **opposite on purpose** — `write_pump`
///    propagates any write error to a non-zero exit, which is right for RPC (a severed protocol
///    stream is a real failure) and wrong for ACP, where the client closing the pipe **is** the
///    normal termination. pi-acp holds the same rule with three guards of its own: an
///    already-destroyed stdout resolves immediately, the write callback's `err` is explicitly
///    discarded, a synchronous `ERR_STREAM_DESTROYED` throw is caught, and any `error` event on
///    stdout exits **0**. The cost is that a genuine `EIO` on the output fd is now indistinguishable
///    from a client hanging up; that is the trade, and it is exit-code fidelity against a
///    supervising editor rather than anything lost or corrupted.
/// 3. **`ACP-024` — the stdin split, pinned.** A clean EOF is exit 0; so is a read error, because
///    `Stdio`'s reader is a `blocking::Unblock` thread whose `read(2)` failure reaches this function
///    as the same connection-closed condition and cannot be distinguished from EOF without
///    reimplementing the transport. Upstream registers `stdin.on('error')` separately and also exits
///    0 from it, so all three paths agreeing on 0 matches pi-acp; it is pinned here so a porter
///    cannot flatten or split it by accident.
///
/// # Errors
///
/// Only a transport fault that is not a hang-up.
pub async fn run_acp_dispatch(host: Arc<dyn cyrup_acp::AcpHost>) -> anyhow::Result<()> {
    match cyrup_acp::serve_stdio(host).await {
        Ok(()) => Ok(()),
        // `ACP-004` — the client closed the pipe. Not a failure; the normal termination.
        Err(cyrup_acp::AcpError::Transport(err)) if is_client_hangup(&err) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Is this transport error the client having gone away? (`ACP-004`.)
///
/// The ACP crate surfaces the `io::Error` inside its own `Error`'s message rather than as a typed
/// kind (`rg 'BrokenPipe'` over the crate returns nothing), so this matches on the two kinds'
/// `Display` text. That is a string test in a port whose whole point is to stop classifying by
/// string, so it is confined to exactly one predicate with one test and it is checked against
/// `io::ErrorKind`'s own `Display`, never against a hand-typed literal — which is what keeps it
/// honest if the standard library ever rewords them.
fn is_client_hangup(err: &cyrup_acp::WireError) -> bool {
    let message = err.message.as_str();
    [
        std::io::ErrorKind::BrokenPipe,
        std::io::ErrorKind::NotConnected,
        std::io::ErrorKind::UnexpectedEof,
    ]
    .into_iter()
    .any(|kind| message.contains(&std::io::Error::from(kind).to_string()))
}

/// The process exit code for a settled one-shot run — pi's `print-mode.ts:139-148`, reproduced
/// exactly: look at the **last** transcript message (not the last ASSISTANT message), and raise the
/// code to 1 only when it is an assistant message whose `stopReason` is `error` or `aborted`.
/// Everything else keeps the `exitCode = 0` pi initialises at `:35`.
///
/// SEAM-016. Three divergences lived here and all three are gone:
/// * the reverse scan (`.iter().rev()`) walked PAST a trailing non-assistant message to an older
///   assistant one, so a run ending in a `Custom` bash message (which `flush_pending_bash_messages`
///   appends) could exit non-zero while `run_print` printed nothing, or exit zero on a stale
///   message;
/// * `Aborted => 130` — pi folds `aborted` into the same `exitCode = 1` as `error` (`:145-147`);
/// * `Pending => 1` — pi has no such arm; an unsettled last message falls to the `else` and keeps 0.
///
/// This is the shared decision function; [`cyrup_modes::run_print`] applies it inline in
/// pi's own block, and this remains for embedders holding a bare [`AgentSession`].
pub async fn exit_code(session: &AgentSession) -> i32 {
    use cyrup_sdk::core::{Message, StopReason};
    match session.messages().await.last() {
        Some(Message::Assistant(assistant)) => match assistant.stop_reason {
            StopReason::Error | StopReason::Aborted => 1,
            StopReason::Stop
            | StopReason::Length
            | StopReason::ToolUse
            | StopReason::Deferred
            | StopReason::Pending => 0,
        },
        _ => 0,
    }
}

/// Wrap one-shot text as a CLI-sourced [`UserInput`].
fn cli_input(text: &str) -> UserInput {
    UserInput::text(text.to_string(), InputSource::Cli)
}

/// The ordered turn a one-shot run submits: the initial submission **when there is one**, then each
/// CLI follow-up (Pi `if (initialMessage) { await session.prompt(…) }` followed by
/// `for (const message of messages) { … }`, print-mode.ts:121-127).
///
/// The `if (initialMessage)` guard is the whole point: `buildInitialMessage` returns
/// `initialMessage: undefined` when there is no stdin, no `@file` and no message
/// (initial-message.ts:36-42), and nothing upstream treats that as fatal — pi skips BOTH loops and
/// falls straight through to the terminal output block (print-mode.ts:129-146), printing the last
/// assistant message of the resumed transcript and returning the `exitCode = 0` it initialised at
/// :34. `cyrup -c -p` (continue a session and print its last response) is that idiom. cyrup used to
/// reject a prompt-less one-shot run outright, via a `main.rs::ensure_prompt` bail that carried no
/// upstream citation, so the exit code inverted (0 ⇒ 1) and JSON mode never even emitted the session
/// header pi writes before `rebindSession()` (print-mode.ts:112-118).
///
/// Emptiness is [`Inputs::is_empty`], i.e. no text **and** no images: an image-only run still
/// submits its initial input (pi's `if (initialMessage)` would drop the images with it, since
/// `initialImages` rides along on that same call — a quirk not worth porting).
fn turn_inputs(inputs: &Inputs) -> Vec<UserInput> {
    let mut turn: Vec<UserInput> = Vec::new();
    if !inputs.is_empty() {
        turn.push(initial_input(inputs));
    }
    turn.extend(inputs.follow_ups.iter().map(|f| cli_input(f)));
    turn
}

/// The initial submission: the assembled text plus any image `@file` attachments (Pi
/// `initialImages`, initial-message.ts:41).
pub fn initial_input(inputs: &Inputs) -> UserInput {
    let mut input = UserInput::text(inputs.initial.clone(), InputSource::Cli);
    input.images = inputs.images.clone();
    input
}
