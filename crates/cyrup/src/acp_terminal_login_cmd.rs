//! `--terminal-login` — the argv gate an ACP client uses to send the user to a real terminal to
//! configure credentials (ACP-001).
//!
//! Port of pi-acp v0.0.33 `src/index.ts`'s top-level
//! `if (process.argv.includes("--terminal-login")) { … }` block, which runs **before any other
//! work**: before the transport exists, before a byte reaches stdout, and — here — before clap.
//!
//! # Why the predicate differs from its three siblings
//!
//! [`crate::subagent_runner_cmd::is_selected`], [`crate::intercom_broker_cmd::is_selected`] and
//! [`crate::mcp_keyring_helper_cmd::is_selected`] are all `argv.get(1) == Some(SUBCOMMAND)` — the
//! token is minted by cyrup itself at a known position. This one is
//! `argv.iter().skip(1).any(…)`, matching upstream's `process.argv.includes`, because the token is
//! appended by somebody else: an ACP client takes the agent command it already has (whatever
//! `command` + `args` it was configured with) and appends `AuthMethod.args` to it, so
//! `--terminal-login` arrives **last**, after every flag the user's editor configuration carries.
//! A positional check would silently never fire and the Authenticate button would do nothing.
//!
//! # [CYRUP-DELTA] — in-process, the "run the agent" step is not a spawn
//!
//! **What differs.** Upstream is a separate npm package, so its gate ends in
//! `spawnSync(getPiCommand(), [], { stdio: "inherit" })` — it launches a *different* binary and
//! exits with the child's status. cyrup-acp is the same executable as the TUI, so there is nothing
//! to spawn: this module strips its own tokens out of argv and lets `main` fall through into the
//! ordinary launch path, which resolves [`cyrup_config::AppMode::Interactive`] by TTY probing
//! exactly as a bare `cyrup` would.
//!
//! **What it costs.** The re-entry is a *fall-through*, not a fresh process, so any pre-clap state
//! `main` has already established (the process name, the bootstrap HTTP proxy) is shared with the
//! login run rather than being re-derived. That is a difference in kind from upstream's clean child
//! and is the reason the strip is a total function over argv rather than a mutation of a resolved
//! [`cyrup_config::AppMode`]: forcing the mode would leave `--acp` live in the parsed [`crate::Cli`]
//! for every downstream reader. See [`strip`].
//!
//! **ACP-Q2, decided here.** Upstream runs `pi` with **no arguments** on the assumption that the
//! user types `/login` at the prompt; cyrup could instead land directly in
//! `cyrup_config::login::resolve_login_command`. This module takes the **parity** answer — relaunch
//! interactively and let the user type `/login` — because it is the behaviour a Zed user who has
//! used pi-acp already expects, and because cyrup has no `login` subcommand today for the
//! alternative to target (that is ACP-Q3, and `ACP-011` keeps `args` naming this flag rather than a
//! subcommand that does not exist). The cost is one extra keystroke for the user; the benefit is
//! that the terminal they land in is the full agent, from which `/model`, `/logout` and the
//! provider picker are also reachable — which is what a credential-less first run usually needs.
//!
//! Never advertised: absent from `--help` and from `crate::subcommands::SUBCOMMANDS`. It is not
//! `__`-prefixed like its three siblings because it is not internal — it is a token cyrup publishes
//! to ACP clients in `AuthMethod::Terminal.args` (`ACP-011`) and is therefore part of a documented
//! wire contract, even though no human is expected to type it.

use std::io::IsTerminal;

/// The literal argv token. Kept **independent** from `cyrup_acp`'s copy in the
/// `AuthMethod::Terminal.args` builder and cross-tested against it rather than shared as one
/// constant across crates — the precedent documented on
/// [`crate::subagent_runner_cmd::SUBCOMMAND`]: "what the client is told to send" and "what `main`
/// recognises" must each be free-standing enough to unit-test without the other. `ACP-011`.
pub const SUBCOMMAND: &str = "--terminal-login";

/// `--mode acp`'s value token, and the `--acp` alias — the two spellings [`strip`] must also
/// remove. See [`strip`] for why.
const ACP_ALIAS: &str = "--acp";
const MODE_FLAG: &str = "--mode";
const MODE_ACP_VALUE: &str = "acp";
const MODE_ACP_INLINE: &str = "--mode=acp";

/// The diagnostic printed when the login gate is reached with no terminal to land in (ACP-026).
///
/// # [CYRUP-DELTA] — a string with no upstream
///
/// **What differs.** Upstream has no such message: its `spawnSync(cmd, [], {stdio:"inherit"})`
/// hands the child whatever stdio the adapter had, so a client that launched the login command with
/// pipes on both ends gets `pi`'s TUI painted into a pipe and no diagnostic at all.
///
/// **What it costs.** A client relying on that (silent) behaviour now sees a stderr line and exit
/// 1 instead of a hung, unreadable child. That is the trade ACP-026 asks for, and it is stated here
/// rather than left implicit because the string is user-visible.
pub const NO_TERMINAL_MESSAGE: &str =
    "cyrup: --terminal-login needs an interactive terminal (stdin and stdout must both be a TTY).\n\
     Run `cyrup` yourself in a terminal and use /login to configure credentials.";

/// Does `argv` — the process's own args **including** the binary name at index 0, matching
/// [`std::env::args`]'s shape — select the terminal-login gate?
///
/// Membership **anywhere** after the binary name, per upstream's `process.argv.includes`. A bare
/// `--terminal-login` as `argv[0]` is not a selection (that is the program name, not an argument),
/// which is the one way this differs from a literal `includes` over Node's argv, where `argv[0]` is
/// the interpreter.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|a| a == SUBCOMMAND)
}

/// `Some(exit_code)` when the gate cannot be honoured because there is no terminal to land in;
/// `None` when it is safe to fall through into the interactive launch (ACP-026).
///
/// The guard is here rather than in `main`'s mode resolution for the reason ACP-026 names: forcing
/// [`cyrup_config::AppMode::Interactive`] would paint a TUI into a pipe. The check is on the real
/// process stdio because that is exactly what the fallen-through interactive launch will take over.
#[must_use]
pub fn refuse_when_not_a_terminal() -> Option<i32> {
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return None;
    }
    eprintln!("{NO_TERMINAL_MESSAGE}");
    Some(1)
}

/// Remove every token that selects the ACP host or the login gate, so the surviving argv is what a
/// bare interactive `cyrup` would have been handed.
///
/// Three token families are removed, and the second and third are the non-obvious half of the unit:
///
/// * `--terminal-login`, which clap does not know and which would otherwise become an
///   `Unknown option` error (`crate::diagnostics::apply_arg_leniency`);
/// * `--acp`, and
/// * `--mode acp` / `--mode=acp`,
///
/// because an ACP client appends `AuthMethod.args` to **the command it already has** — which is the
/// ACP agent command, so `--acp` (or `--mode acp`) is virtually always still present. Leaving
/// either would make `crate::cli::resolve_app_mode` answer `Acp` for a run whose entire purpose is
/// to be a TUI, and its first branch (ACP-002) would win over the TTY probe.
///
/// A `--mode` carrying any other value is preserved verbatim, value token included: this function
/// strips the ACP selection, not the user's mode choice. A trailing bare `--mode` with no value is
/// left for clap to complain about, rather than being silently swallowed here.
#[must_use]
pub fn strip(argv: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut iter = argv.into_iter();
    // argv[0] is the program name and is never inspected.
    if let Some(program) = iter.next() {
        out.push(program);
    }
    while let Some(arg) = iter.next() {
        if arg == SUBCOMMAND || arg == ACP_ALIAS || arg == MODE_ACP_INLINE {
            continue;
        }
        if arg == MODE_FLAG {
            // Peek the value: drop the pair only when it is exactly `acp`.
            match iter.next() {
                Some(value) if value == MODE_ACP_VALUE => continue,
                Some(value) => {
                    out.push(arg);
                    out.push(value);
                }
                // A trailing `--mode` with no value: hand it to clap unchanged.
                None => out.push(arg),
            }
            continue;
        }
        out.push(arg);
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        std::iter::once("cyrup")
            .chain(tokens.iter().copied())
            .map(str::to_string)
            .collect()
    }

    /// ACP-001's *Verify*, first half: membership at every position 1..n, and absence.
    ///
    /// The `--acp --terminal-login` row is the one that matters — it is the literal shape a client
    /// produces when it appends `AuthMethod.args` to the agent command it already holds.
    #[test]
    fn terminal_login_is_recognised_anywhere_after_the_program_name() {
        assert!(is_selected(&argv(&["--terminal-login"])));
        assert!(is_selected(&argv(&["--acp", "--terminal-login"])));
        assert!(is_selected(&argv(&["--terminal-login", "--acp"])));
        assert!(is_selected(&argv(&[
            "--acp",
            "--session-dir",
            "/tmp/x",
            "--terminal-login"
        ])));
        assert!(!is_selected(&argv(&[])));
        assert!(!is_selected(&argv(&["--acp"])));
        assert!(!is_selected(&argv(&["--terminal-logins"])));
        assert!(!is_selected(&argv(&["terminal-login"])));
        // argv[0] is the program name, never an argument — a binary literally named
        // `--terminal-login` does not select the gate.
        assert!(!is_selected(&["--terminal-login".to_string()]));
    }

    /// The strip is what keeps `resolve_app_mode` from answering `Acp` for the login run, and it
    /// must not eat an unrelated `--mode`.
    #[test]
    fn strip_removes_the_gate_and_every_acp_selector_but_nothing_else() {
        assert_eq!(strip(argv(&["--acp", "--terminal-login"])), argv(&[]));
        assert_eq!(
            strip(argv(&["--mode", "acp", "--terminal-login"])),
            argv(&[])
        );
        assert_eq!(strip(argv(&["--mode=acp", "--terminal-login"])), argv(&[]));
        assert_eq!(
            strip(argv(&["--terminal-login", "--session-dir", "/tmp/x"])),
            argv(&["--session-dir", "/tmp/x"])
        );
        // A different `--mode` survives with its value.
        assert_eq!(
            strip(argv(&["--mode", "json", "--terminal-login"])),
            argv(&["--mode", "json"])
        );
        // A trailing bare `--mode` is clap's problem, not this function's.
        assert_eq!(strip(argv(&["--terminal-login", "--mode"])), argv(&["--mode"]));
        // Idempotent, and a no-op on an argv that never selected the gate.
        let plain = argv(&["--json", "hello"]);
        assert_eq!(strip(plain.clone()), plain);
        assert_eq!(strip(strip(argv(&["--acp", "--terminal-login"]))), argv(&[]));
    }

    /// ACP-011's cross-test: the token this crate recognises is byte-identical to the one
    /// `cyrup_acp` publishes in `AuthMethod::Terminal.args`. The two constants are deliberately
    /// separate declarations, so this assertion is what keeps them in step.
    #[test]
    fn the_recognised_token_is_the_one_advertised_to_acp_clients() {
        assert_eq!(SUBCOMMAND, cyrup_acp::TERMINAL_LOGIN_ARG);
        assert_eq!(SUBCOMMAND, "--terminal-login");
    }
}
