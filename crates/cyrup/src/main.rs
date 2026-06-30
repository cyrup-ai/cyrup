//! cyrup — the CLI binary (arch-11 §2.4). The sole `anyhow` boundary and the only binary in the
//! workspace.
//!
//! Thin by design: parse args, initialise tracing to **stderr** (so stdout stays clean for
//! PRINT/JSON/RPC), probe the TTYs, resolve config directories, map the CLI to a `SessionConfig`,
//! select a provider, build the one `AgentSession` seam, install signal handling, then dispatch to
//! the resolved runtime mode. All reusable logic lives in the `cyrup` library so it is testable
//! without a TTY; only the interactive `CrosstermBackend` wiring stays here (it needs a real
//! terminal and is not unit-tested).

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use cyrup::{
    build_inputs, resolve_app_mode, run_json_dispatch, run_print_dispatch, run_rpc_dispatch,
    select_provider, spawn_abort_on_signal, AppMode, Cli, Inputs,
};
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars};
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, InputSource, SessionBuilder, SessionFactory, UserInput,
};
use cyrup_tui::{crossterm_input_stream, App, UiTheme};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            // The single anyhow boundary: report the full cause chain to stderr, exit non-zero.
            eprintln!("cyrup: {err:#}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let stdin_tty = io::stdin().is_terminal();
    let stdout_tty = io::stdout().is_terminal();
    let mode = resolve_app_mode(&cli, stdin_tty, stdout_tty);

    // Resolve directories (CLI > env > default; the only place env is read).
    let env = EnvVars::from_process();
    let overrides = CliConfigOverrides { cwd: cli.cwd.clone(), ..Default::default() };
    let dirs = ConfigDirs::resolve(&overrides, &env).context("resolving config directories")?;

    // Map CLI → SessionConfig, pick a provider.
    let config = cli.to_session_config(&dirs, mode);
    let provider = select_provider(cli.model.as_deref())?;
    let cancel = CancelToken::new();

    // Interactive mode drives the **multi-session** `AgentSessionRuntime` (arch-11 §3.4) so the
    // session-swap commands (`/new`, `/resume`, `/fork`, `/reload`, `/import`) rebuild the active
    // session in place and the TUI re-binds to it (Pi `agent-session-runtime.ts` + interactive-mode
    // session-swap). The one-shot/RPC modes keep the single fixed `AgentSession` seam unchanged: they
    // never swap sessions, so the runtime tier is not built for them and their flow is unaffected.
    if let AppMode::Interactive = mode {
        let target = config.target.clone();
        let factory = Arc::new(SessionFactory::new(provider, config));
        let runtime = Arc::new(
            AgentSessionRuntime::create(factory, target)
                .await
                .context("building agent session runtime")?,
        );
        let session = runtime.session().await;
        // Signals: SIGINT/SIGTERM → abort the run + cancel the interactive loop (R-11-010/018). The
        // abort targets the session active at launch; the cancel token breaks the loop on any swap.
        let _signals = spawn_abort_on_signal(session.clone(), cancel.clone());
        let inputs = build_inputs(&cli).await?;
        run_interactive(runtime, session, inputs, cancel).await?;
        return Ok(0);
    }

    match mode {
        AppMode::Rpc => {
            // RPC drives the **multi-session** `AgentSessionRuntime` host (Pi `rpc-mode.ts`
            // `runtimeHost`) so the session-replacing commands (`new_session`/`switch_session`/
            // `fork`/`clone`) rebuild the active session in place and the protocol rebinds. RPC owns
            // stdin as the protocol; do not pre-read prompt inputs.
            let target = config.target.clone();
            let factory = Arc::new(SessionFactory::new(provider, config));
            let runtime = Arc::new(
                AgentSessionRuntime::create(factory, target)
                    .await
                    .context("building agent session runtime")?,
            );
            let session = runtime.session().await;
            let _signals = spawn_abort_on_signal(session, cancel.clone());
            let reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            run_rpc_dispatch(&runtime, reader, &mut writer).await?;
            Ok(0)
        }
        AppMode::Print | AppMode::Json => {
            // One-shot modes never swap sessions: build the one `AgentSession` seam (R-11-008).
            let session = Arc::new(
                SessionBuilder::new(provider, config)
                    .build()
                    .await
                    .context("building agent session")?,
            );
            let _signals = spawn_abort_on_signal(session.clone(), cancel.clone());
            let inputs = build_inputs(&cli).await?;
            ensure_prompt(&inputs)?;
            let mut out = io::stdout();
            if let AppMode::Json = mode {
                run_json_dispatch(&session, &inputs, &mut out).await
            } else {
                run_print_dispatch(&session, &inputs, &mut out).await
            }
        }
        // Interactive is dispatched above (it builds the runtime tier) and returns early.
        AppMode::Interactive => unreachable!("interactive mode is handled before this match"),
    }
}

/// Require a non-empty prompt for the one-shot modes (a message, an `@file`, or piped stdin).
fn ensure_prompt(inputs: &Inputs) -> anyhow::Result<()> {
    if inputs.is_empty() {
        anyhow::bail!(
            "no prompt provided: pass a message, an @file reference, or pipe text on stdin"
        );
    }
    Ok(())
}

/// The interactive front-end: build the TUI over a real `CrosstermBackend<Stdout>`, seed any initial
/// prompt, and run the event loop against the live session. Restores the terminal on exit. Thin and
/// not unit-tested (it requires a real TTY); the testable logic lives in the library.
async fn run_interactive(
    runtime: Arc<AgentSessionRuntime>,
    session: Arc<AgentSession>,
    inputs: Inputs,
    cancel: CancelToken,
) -> anyhow::Result<()> {
    let mut app = App::into_stdout(UiTheme::default()).context("initialising the terminal UI")?;
    // Probe the controlling TTY for its real image protocol (Kitty/iTerm2/sixel), upgrading from the
    // portable half-block default so inline images render as native graphics where supported
    // (spec/tui/06 §6; `terminal-image.ts`). Degrades silently to half-blocks on unsupported terminals.
    app.detect_image_support();
    let input_stream = crossterm_input_stream(cancel.clone());
    let events = session.subscribe();

    if !inputs.is_empty() {
        app.state_mut().transcript.push_user(inputs.initial.clone());
        let _ = session
            .prompt_accepted(UserInput::text(inputs.initial.clone(), InputSource::Cli))
            .await;
    }

    // Hand the runtime to the loop so the session-swap commands rebuild the active session and the UI
    // re-binds (re-subscribes the event stream + resets the transcript) on the generation bump.
    let result =
        app.run(input_stream, events, session.clone(), Some(runtime), None, cancel).await;
    // Total + idempotent restore so an error path still leaves a usable terminal.
    let _ = app.restore();
    result.map_err(|e| anyhow::anyhow!("tui: {e}"))?;
    Ok(())
}

/// Initialise `tracing` to **stderr** (off the protocol stream), honouring `RUST_LOG`. Off by default
/// (only `warn`); `-v` raises the floor to `debug`. Idempotent and never fatal.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt().with_env_filter(filter).with_writer(io::stderr).try_init();
}
