use cyrup_config::AppMode;

use super::args::Cli;
use super::enums::{Mode, OutputFormat};

/// Resolve the runtime mode (R-11-001 / arch-11 §6.1). Explicit mode flags win; otherwise a
/// non-TTY stdin or stdout forces PRINT, and a full TTY pair selects the interactive front-end.
/// `--mode text` is the DEFAULT (it does not force PRINT by itself — only `--print`/non-TTY does).
pub fn resolve_app_mode(cli: &Cli, stdin_tty: bool, stdout_tty: bool) -> AppMode {
    // ACP-002 — FIRST, and the position is the whole unit. An ACP agent is launched by an editor
    // with pipes on both ends, so `!stdin_tty || !stdout_tty` is always true for it. Any branch
    // ahead of this one that can match would resolve `Print`, and the host would then read the
    // client's first JSON-RPC frame as a chat prompt and answer it as one-shot text on the stream
    // the client is parsing as JSON-RPC. Nothing downstream can recover from that, which is why it
    // is ordered rather than merely present.
    if cli.acp || cli.mode == Some(Mode::Acp) {
        return AppMode::Acp;
    }
    if cli.rpc || cli.mode == Some(Mode::Rpc) {
        return AppMode::Rpc;
    }
    if cli.json || cli.mode == Some(Mode::Json) || cli.output_format == Some(OutputFormat::Json) {
        return AppMode::Json;
    }
    if cli.print || cli.output_format == Some(OutputFormat::Text) {
        return AppMode::Print;
    }
    if !stdin_tty || !stdout_tty {
        return AppMode::Print;
    }
    AppMode::Interactive
}

/// `isPlainRuntimeMetadataCommand` (main.ts:117-119): a non-`--print`, no-`--mode`, `--help`-or-
/// `--list-models` invocation. Such commands keep stdout pristine for their own output (they are NOT
/// stdout-guarded).
pub fn is_plain_runtime_metadata_command(cli: &Cli) -> bool {
    !cli.print && cli.mode.is_none() && (cli.help || cli.list_models.is_some())
}

/// `shouldTakeOverStdout` (main.ts:535): take over stdout for non-interactive modes that are not a
/// plain metadata command, so stray library writes cannot pollute the PRINT/JSON/RPC stream. (In
/// Rust the streams the bin owns are already disciplined — tracing goes to stderr — so the takeover
/// itself is a no-op; only the Pi-faithful DECISION is modelled and tested here.)
pub fn should_take_over_stdout(cli: &Cli, mode: AppMode) -> bool {
    mode != AppMode::Interactive && !is_plain_runtime_metadata_command(cli)
}
