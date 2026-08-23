//! `__intercom-broker` — the hidden, never-user-facing CLI subcommand that runs the standalone
//! intercom broker process (a 1:1 analog of the `__subagent-runner` hop, `subagent_runner_cmd.rs`).
//!
//! The per-session intercom extension auto-spawns the broker by re-execing `current_exe()` (the
//! `cyrup` binary) with this subcommand, as a genuinely detached process
//! (`cyrup_intercom::transport::spawn::ensure_broker`). This module recognizes that argv and hands
//! straight to [`cyrup_intercom::broker::run`], which binds the Unix socket at
//! `<intercomDir>/broker.sock`, runs the accept/route loop, answers the health probe, and auto-shuts
//! down 5 s after its last client leaves.
//!
//! Never advertised to users: not listed in `--help`, not one of `crate::subcommands::SUBCOMMANDS`,
//! and dispatched from `main()` BEFORE any user-facing arg leniency/clap parsing runs — mirroring the
//! `__subagent-runner` pre-dispatch's placement.
//!
//! # [CYRUP-DELTA] (SEAM-109) — an argv verb has no upstream counterpart at all
//!
//! **pi has NO argv verbs**; see [`crate::subagent_runner_cmd`]'s delta for the check that
//! establishes it (`git -C pi grep -nE 'argv\[2\]|process\.argv' v0.83.0` → nothing, and
//! `rpc-entry.ts` / `bun/cli.ts` are separate entry points, not verbs).
//!
//! **The mechanism this replaces** is `pi-intercom`'s broker launch: `broker/spawn.ts:157-163`
//! resolves the command as `node <tsx-cli> <brokerPath>` and `getBrokerSpawnOptions` (`:174-191`)
//! spawns it `detached: true` with `stdio: "ignore"` and `PI_CODING_AGENT_DIR` in the child env —
//! read at `pi-intercom` HEAD `30dcbdd`. As with the subagent runner, upstream's selector is a
//! separate SCRIPT PATH handed to an interpreter. cyrup is one compiled binary, so the broker
//! PROCESS — which is the load-bearing part: a standalone Unix-socket hub that outlives any one
//! session — is re-exec'd out of `current_exe()` under a reserved argv token instead. Detachment,
//! the agent-dir env handover and the socket protocol are ported literally.
//!
//! **Deliberately undocumented:** `__`-prefixed, absent from `--help` and
//! `crate::subcommands::SUBCOMMANDS`, matched only as an exact `argv[1]` ([`is_selected`]).
//! Undiscoverable rather than absent — recorded so the invented surface is KNOWN, not assumed parity.

/// The literal `argv[0]` token identifying this internal subcommand (mirrors
/// `cyrup_intercom::transport::spawn::INTERCOM_BROKER_SUBCOMMAND`).
pub const SUBCOMMAND: &str = "__intercom-broker";

/// Returns `true` if `argv` (the process's own args, *including* `argv[0]`/the binary name at index 0,
/// matching [`std::env::args`]'s shape) selects this internal subcommand.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.get(1).map(String::as_str) == Some(SUBCOMMAND)
}

/// Run the broker to completion and return the process exit code `main` should use. The broker reads
/// its agent dir from `CYRUP_CODING_AGENT_DIR` (injected by the spawner) and cleans up its runtime
/// files on shutdown; any bind/IO failure is reported to stderr with a non-zero exit.
pub async fn dispatch() -> i32 {
    match cyrup_intercom::broker::run().await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{SUBCOMMAND}: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn is_selected_matches_the_exact_internal_subcommand_token() {
        assert!(is_selected(&["cyrup".to_string(), "__intercom-broker".to_string()]));
        assert!(!is_selected(&["cyrup".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "__subagent-runner".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "--help".to_string()]));
        assert!(!is_selected(&[]));
    }

    /// SEAM-109 — the two hidden argv verbs are cyrup inventions (pi has no argv verbs at all), so
    /// two things are owed and both are asserted here.
    ///
    /// 1. They stay INVISIBLE. `--help` and the package-subcommand table must not name either — the
    ///    verbs are internal re-exec selectors, not a user surface, and the moment one leaks into the
    ///    help it becomes a command a user can reasonably expect to keep working.
    /// 2. Each carries a `[CYRUP-DELTA]` naming the upstream mechanism it replaces. RED before this
    ///    pass: neither module had one, so a reader had no way to tell an invented surface from a
    ///    ported one. Read from source at compile time, the only way to assert on a doc block.
    #[test]
    fn the_hidden_argv_verbs_are_invisible_and_declared_as_cyrup_deltas() {
        let help = crate::cli::render_help(&[]);
        for token in [SUBCOMMAND, crate::subagent_runner_cmd::SUBCOMMAND] {
            assert!(!help.contains(token), "`{token}` must not appear in --help");
            assert!(
                crate::subcommands::first_subcommand(&[token.to_string()]).is_none(),
                "`{token}` must not be one of the package/config SUBCOMMANDS"
            );
        }

        for (module, src, upstream) in [
            (
                "intercom_broker_cmd",
                include_str!("intercom_broker_cmd.rs"),
                "broker/spawn.ts",
            ),
            (
                "subagent_runner_cmd",
                include_str!("subagent_runner_cmd.rs"),
                "async-execution.ts",
            ),
        ] {
            let delta = src
                .lines()
                .position(|l| l.contains("[CYRUP-DELTA]") && l.contains("SEAM-109"))
                .unwrap_or_else(|| panic!("{module} must carry a [CYRUP-DELTA] naming SEAM-109"));
            let block: String =
                src.lines().skip(delta).take(24).collect::<Vec<_>>().join("\n");
            assert!(
                block.contains(upstream),
                "{module}'s delta must name the upstream mechanism it replaces ({upstream}): {block}"
            );
            assert!(
                block.contains("no argv verbs") || block.contains("NO argv verbs"),
                "{module}'s delta must state that pi has no argv verbs at all: {block}"
            );
        }
    }
}
