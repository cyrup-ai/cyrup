//! Argv routing that must run **before** clap ever sees the command line.
//!
//! Six gates run ahead of `parseArgs`, in pi's own order, and each can end the process:
//!
//! 1. the internal `__subagent-runner --config <path>` hop (arch-SA §2.2/§6.5; func-SA §1.1),
//! 2. the internal `__subagent-inspector --async-dir <dir> --run-id <id>` hop (the inspector pane
//!    a terminal host starts from the string `inspector.command` returns — VL-S6),
//! 3. the internal `__intercom-broker` hop (spec/extensions/cyrup-intercom-port.md §7.3),
//! 4. the internal `__mcp-keyring-helper` hop (13f-mcp-credentials MCP-260/MCP-261),
//! 5. the package/config subcommands (pi `handlePackageCommand`, main.ts:486),
//! 6. `auth print-api-key|print-bearer-token` (pi `runCredentialPrintCommand`, main.ts:557-559).
//!
//! The four internal hops are **classified** here and **dispatched** by `main.rs`, rather than
//! being dispatched here, for one specific reason: each one first re-labels the process
//! (`cyrup-subagent` / `cyrup-inspector` / `cyrup-broker` / `cyrup-mcp-keyring`, SEAM-070) and
//! `set_process_name` needs `unsafe` (`prctl(PR_SET_NAME)` / `pthread_setname_np`), which this
//! crate's `#![forbid(unsafe_code)]` rules out. Splitting classification from dispatch keeps the
//! `unsafe` in the binary — where the rest of the process-identity work already lives — without
//! threading a callback through this module.
//!
//! **A classification arm with no matching `main.rs` dispatch arm fails SILENTLY**: the argv falls
//! through to clap, which rejects the hop's own flags with a usage error and exit 2. That is why
//! each hop is covered end to end by a test that spawns the REAL binary — for the inspector,
//! `crates/cyrup-it/tests/subagents/inspector_runner_subcommand_integration.rs`.

use anyhow::Context;
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars};

use crate::{
    acp_terminal_login_cmd, credential_print, intercom_broker_cmd, mcp_keyring_helper_cmd,
    subagent_inspector_cmd, subagent_runner_cmd, subcommands,
};

/// Which internal, never-advertised subcommand this argv selects, if any.
///
/// None of the four appears in `--help` or in `subcommands::SUBCOMMANDS`. All four MUST be
/// recognized before ANY user-facing arg leniency/clap parsing — and before the package/config
/// pre-dispatch, which has no knowledge of them and would otherwise fall through to ordinary clap
/// parsing, misinterpreting `--config <path>` (or `--async-dir <dir>`) against the user-facing
/// `Cli` surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    /// `__subagent-runner --config <path>` — hop 2 of the SubAgents extension's mandated
    /// background-execution mechanism.
    SubagentRunner,
    /// `__subagent-inspector --async-dir <dir> --run-id <id> …` — the inspector PANE (VL-S6).
    ///
    /// Unlike [`Self::SubagentRunner`], cyrup never spawns this hop. `inspector.command` returns
    /// the launch string and `inspector.open` hands it to a TERMINAL HOST (`herdr pane run`,
    /// Ghostty's AppleScript), which executes it; upstream does the same with
    /// `inspector-runner.mjs` (`src/inspectors/actions.ts:90-104` @v0.68.0). The process it
    /// becomes is a read-only mirror of one async run: it renders a dashboard on a timer and
    /// reads `status` / `stop` / `steer <message>` / plain guidance off stdin.
    ///
    /// Its argv — `--async-dir`, `--run-id`, `--allow-steer`, `--allow-stop`, `--session-roots`,
    /// `--index`, `--mission-path`, `--refresh-ms` — shares NO flag spelling with the
    /// user-facing `Cli` surface, so reaching clap with it is a usage error rather than a
    /// misinterpretation. That makes the half-wired failure loud at the process boundary and
    /// invisible everywhere else, which is what the end-to-end test exists for.
    SubagentInspector,
    /// `__intercom-broker` — the hidden subcommand the per-session intercom extension re-execs
    /// `current_exe()` into to stand up the standalone broker PROCESS (a Unix-socket hub). Its
    /// `--config`-free argv must never reach the user-facing `Cli` surface.
    IntercomBroker,
    /// `__mcp-keyring-helper` — the hidden subcommand
    /// [`cyrup_mcp::credentials::McpAuthStore`]'s Linux keyring-recovery arm re-execs
    /// `current_exe()` into, as `<keyctl> session - <cyrup> __mcp-keyring-helper`, when the session
    /// keyring has been revoked and an MCP OAuth credential read/write/remove must be retried
    /// inside a fresh anonymous session keyring (MCP-260). The child speaks one line of JSON on
    /// stdin and one on stdout, so it must reach [`crate::mcp_keyring_helper_cmd::dispatch`] before
    /// anything can log, print, or otherwise put a byte on stdout.
    McpKeyringHelper,
    /// `--terminal-login` — the ACP client's Authenticate button (ACP-001). Port of pi-acp v0.0.33
    /// `index.ts`'s top-level `process.argv.includes("--terminal-login")` block.
    ///
    /// **The odd one out in three ways, and each is load-bearing.** (1) Its predicate is membership
    /// ANYWHERE after the program name, not `argv[1]`, because an ACP client appends
    /// `AuthMethod.args` to a command it already holds — see
    /// [`crate::acp_terminal_login_cmd::is_selected`]. (2) It is not `__`-prefixed and is not
    /// internal: it is a token cyrup publishes to clients in `AuthMethod::Terminal.args`
    /// (`ACP-011`). (3) It does **not** end the process — `main` strips its tokens and falls
    /// through into the ordinary interactive launch, which is why it is classified LAST of the five
    /// (a `__subagent-runner` whose own argv happens to contain the token must still be a subagent
    /// runner).
    AcpTerminalLogin,
}

/// Classify the internal pre-dispatch. `raw` (not the program-stripped `argv`) is passed because
/// all five `is_selected` predicates expect the binary name at index 0, matching
/// `std::env::args()`'s own shape.
pub fn classify_internal(raw: &[String]) -> Option<Internal> {
    if subagent_runner_cmd::is_selected(raw) {
        return Some(Internal::SubagentRunner);
    }
    // VL-S6 — after `__subagent-runner` and before `--terminal-login`. The four `__`-prefixed
    // predicates are mutually exclusive (each is an exact, distinct `argv[1]`), so their relative
    // order is a readability choice; `--terminal-login`'s is NOT, which is why it stays last.
    if subagent_inspector_cmd::is_selected(raw) {
        return Some(Internal::SubagentInspector);
    }
    if intercom_broker_cmd::is_selected(raw) {
        return Some(Internal::IntercomBroker);
    }
    if mcp_keyring_helper_cmd::is_selected(raw) {
        return Some(Internal::McpKeyringHelper);
    }
    // ACP-001 — LAST of the five, because its predicate is membership anywhere in argv rather than a
    // fixed position: checking it first would let a `--terminal-login` appearing in one of the four
    // internal subcommands' own argv hijack that hop.
    if acp_terminal_login_cmd::is_selected(raw) {
        return Some(Internal::AcpTerminalLogin);
    }
    None
}

/// The two user-facing pre-clap gates, in pi's order. `Some(code)` means the run was handled and
/// the caller returns that exit code.
///
/// The package/config subcommands resolve their dirs with NO CLI overrides, for the subcommand's
/// own package/project roots (pi main.ts:486, before arg parsing) — and only when a subcommand is
/// actually present, so an ordinary launch never pays for (or fails on) a resolve it will redo with
/// the real overrides.
///
/// The credential-print gate is pi's `if (await runCredentialPrintCommand(args)) return;`
/// (main.ts:557-559) — immediately after the config/package block and BEFORE `parseArgs`. Without
/// it `auth` is not a known verb, so the tokens survive arg leniency as bare positionals and become
/// a chat PROMPT: no credential, no error, an agent session started and tokens burned on an auth
/// subcommand.
pub async fn run_predispatch(argv: &[String]) -> anyhow::Result<Option<i32>> {
    if subcommands::first_subcommand(argv).is_some() {
        let env = EnvVars::from_process();
        let dirs = ConfigDirs::resolve(&CliConfigOverrides::default(), &env)
            .context("resolving config directories")?;
        let trust_override = subcommands::trust_override(argv);
        if let Some(code) = subcommands::dispatch(argv, &dirs, trust_override).await? {
            return Ok(Some(code));
        }
    }
    if let Some(code) = credential_print::dispatch(argv).await {
        return Ok(Some(code));
    }
    Ok(None)
}
