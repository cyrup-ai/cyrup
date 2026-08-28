//! Argv routing that must run **before** clap ever sees the command line.
//!
//! Five gates run ahead of `parseArgs`, in pi's own order, and each can end the process:
//!
//! 1. the internal `__subagent-runner --config <path>` hop (arch-SA §2.2/§6.5; func-SA §1.1),
//! 2. the internal `__intercom-broker` hop (spec/extensions/cyrup-intercom-port.md §7.3),
//! 3. the internal `__mcp-keyring-helper` hop (13f-mcp-credentials MCP-260/MCP-261),
//! 4. the package/config subcommands (pi `handlePackageCommand`, main.ts:486),
//! 5. `auth print-api-key|print-bearer-token` (pi `runCredentialPrintCommand`, main.ts:557-559).
//!
//! The three internal hops are **classified** here and **dispatched** by `main.rs`, rather than
//! being dispatched here, for one specific reason: each one first re-labels the process
//! (`cyrup-subagent` / `cyrup-broker` / `cyrup-mcp-keyring`, SEAM-070) and `set_process_name` needs
//! `unsafe` (`prctl(PR_SET_NAME)` / `pthread_setname_np`), which this crate's
//! `#![forbid(unsafe_code)]` rules out. Splitting classification from dispatch keeps the `unsafe` in
//! the binary — where the rest of the process-identity work already lives — without threading a
//! callback through this module.

use anyhow::Context;
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars};

use crate::{
    credential_print, intercom_broker_cmd, mcp_keyring_helper_cmd, subagent_runner_cmd, subcommands,
};

/// Which internal, never-advertised subcommand this argv selects, if any.
///
/// None of the three appears in `--help` or in `subcommands::SUBCOMMANDS`. All three MUST be
/// recognized before ANY user-facing arg leniency/clap parsing — and before the package/config
/// pre-dispatch, which has no knowledge of them and would otherwise fall through to ordinary clap
/// parsing, misinterpreting `--config <path>` against the user-facing `Cli` surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    /// `__subagent-runner --config <path>` — hop 2 of the SubAgents extension's mandated
    /// background-execution mechanism.
    SubagentRunner,
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
}

/// Classify the internal pre-dispatch. `raw` (not the program-stripped `argv`) is passed because
/// all three `is_selected` predicates expect the binary name at index 0, matching
/// `std::env::args()`'s own shape.
pub fn classify_internal(raw: &[String]) -> Option<Internal> {
    if subagent_runner_cmd::is_selected(raw) {
        return Some(Internal::SubagentRunner);
    }
    if intercom_broker_cmd::is_selected(raw) {
        return Some(Internal::IntercomBroker);
    }
    if mcp_keyring_helper_cmd::is_selected(raw) {
        return Some(Internal::McpKeyringHelper);
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
