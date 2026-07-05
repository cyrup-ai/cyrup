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
    session: &AgentSession,
    inputs: &Inputs,
    out: &mut W,
) -> anyhow::Result<i32> {
    let messages =
        std::iter::once(initial_input(inputs)).chain(inputs.follow_ups.iter().map(|f| cli_input(f)));
    let mut err = std::io::stderr();
    run_print(session, messages, out, &mut err, PrintOptions::default()).await?;
    Ok(exit_code(session).await)
}

/// JSON dispatch: run the initial prompt then each follow-up, streaming every event as JSONL to `out`.
///
/// JSON mode ALWAYS returns exit 0 (Pi `print-mode.ts:34,129-148`): `exitCode` inits to `0` and is
/// mutated only inside the `if (mode === "text")` branch, so a failed/aborted final turn NEVER changes
/// the JSON-mode exit code — a consumer scripting `cyrup --mode json … ; echo $?` relies on the
/// always-0 convention. (The terminal stop reason is still observable in the streamed event records.)
pub async fn run_json_dispatch<W: Write>(
    session: &AgentSession,
    inputs: &Inputs,
    out: &mut W,
) -> anyhow::Result<i32> {
    run_json(session, initial_input(inputs), out).await?;
    for follow_up in &inputs.follow_ups {
        run_json(session, cli_input(follow_up), out).await?;
    }
    Ok(0)
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
    run_rpc(runtime, reader, writer).await?;
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
