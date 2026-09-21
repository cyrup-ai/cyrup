//! `__subagent-inspector --async-dir <dir> --run-id <id> …` — the internal, never-user-facing CLI
//! subcommand that IS the inspector pane.
//!
//! An inspector is a terminal pane, opened next to the user's session, running a separate process
//! that mirrors ONE async run: it clears the screen, renders a lifecycle dashboard every
//! `--refresh-ms`, and reads control lines (`status`, `stop`, `steer <message>`, plain guidance)
//! off stdin. Closing the pane does not stop the run.
//!
//! **Nothing in cyrup spawns this process.** The verb `inspector.command`
//! ([`cyrup_ext_subagents::inspectors::actions`]) RETURNS the launch string, and
//! `inspector.open` hands that same string to a terminal host — `herdr pane run <paneId> <cmd>`
//! or Ghostty's AppleScript `command of surfaceConfiguration` — which is what actually starts it.
//! So unlike [`crate::subagent_runner_cmd`], this module has no spawning sibling: its ONLY
//! producer is a string a human or a terminal host executes.
//!
//! This module is the thin binary-crate half of the seam. Everything it does beyond selecting
//! itself lives in the library, at
//! [`cyrup_ext_subagents::inspectors::runner::run_inspector`] — argv parsing, the dashboard, the
//! control verbs and the refresh loop, all written against an injected argv/stdin/stdout so they
//! are testable with no TTY and no pane.
//!
//! Never advertised: not in `--help`, not in [`crate::subcommands`]'s list, matched only as an
//! exact `argv[1]`, and dispatched from `main()` before any user-facing arg leniency or clap
//! parsing runs — the same placement, for the same reason, as `__subagent-runner`.
//!
//! # [CYRUP-DELTA] (SEAM-109) — this is that delta's SECOND token
//!
//! The premise recorded at [`crate::subagent_runner_cmd`]'s module doc holds here unchanged and is
//! not restated: pi has no argv verbs, because it hands a separate TypeScript file to `node` +
//! `jiti` and the SCRIPT PATH is the selector. Upstream's inspector launch is
//! `[resolveNodeExecutable(), inspector-runner.mjs, --async-dir, …]`
//! (`src/inspectors/actions.ts:90-104` @v0.68.0); cyrup ships one compiled binary with no
//! interpreter to hand a script to, so the script slot becomes a reserved argv token. Same
//! mechanism, same undiscoverability argument, second token.

use cyrup_ext_subagents::inspectors::runner::run_inspector;

/// The literal `argv[1]` token identifying this internal subcommand.
///
/// A SECOND, INDEPENDENT literal of
/// [`cyrup_ext_subagents::inspectors::types::INSPECTOR_SUBCOMMAND`], deliberately not an import —
/// the convention `cyrup_ext_subagents::background::spawn_detached`'s own doc states for
/// `__subagent-runner`, and for the same reason: "what the launch string contains" and "what
/// `main` recognizes" must each be free-standing enough to unit-test without depending on the
/// other crate's internals. The two are pinned against each other by this module's
/// `the_two_independent_literals_agree` test, which is what makes "a second literal" safe rather
/// than merely duplicated.
pub const SUBCOMMAND: &str = "__subagent-inspector";

/// Returns `true` if `argv` (the process's own args, *including* the binary name at index 0,
/// matching [`std::env::args`]'s shape) selects this internal subcommand.
///
/// An EXACT `argv[1]` match, exactly like [`crate::subagent_runner_cmd::is_selected`] and
/// deliberately unlike [`crate::acp_terminal_login_cmd::is_selected`]'s membership-anywhere
/// predicate: this check must run before any user-facing CLI parsing, because `--async-dir`,
/// `--run-id`, `--allow-steer`, `--allow-stop`, `--session-roots`, `--index`, `--mission-path` and
/// `--refresh-ms` are not part of the user-facing [`crate::cli::Cli`] surface at all. A
/// classification arm added WITHOUT the matching `main.rs` dispatch arm does not fail loudly — the
/// argv falls through to clap, which rejects `--async-dir` with a usage error and exit 2.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.get(1).map(String::as_str) == Some(SUBCOMMAND)
}

/// Run the inspector pane to completion and return the process exit code `main` should use.
///
/// Stdin and stdout are the pane's real ones — this process IS the pane. The library half takes
/// them as parameters so the whole loop is unit-testable against in-memory buffers; here they are
/// the genuine article.
///
/// The only failure surfaced is a pre-flight one: an argv the parser refuses, or a stdout that
/// cannot be written (the pane went away mid-render). Both are reported as upstream reports them —
/// `Inspector failed: {message}` on stderr (`src/inspectors/inspector-runner.ts:150`) plus a
/// non-zero exit. Everything after that point is rendered INSIDE the pane: a refused control line
/// becomes a `Control error: …` notice on the next screen rather than ending the process, because
/// a mirror that dies on a typo is worse than useless to the human watching it.
pub async fn dispatch(argv: &[String]) -> i32 {
    let rest = argv.get(2..).unwrap_or(&[]);
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    match run_inspector(rest, stdin, stdout).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Inspector failed: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    /// The token this crate recognises and the token the launch string embeds must be the same
    /// bytes. Two independent literals, one asserted equality — which is what makes the
    /// "kept as a second literal" convention safe rather than merely duplicated.
    ///
    /// GUT: change [`SUBCOMMAND`] by one character — this goes red, and in production every
    /// `inspector.command` string would name a subcommand the binary does not answer to.
    #[test]
    fn the_two_independent_literals_agree() {
        assert_eq!(
            SUBCOMMAND,
            cyrup_ext_subagents::inspectors::types::INSPECTOR_SUBCOMMAND
        );
    }

    /// GUT: change [`is_selected`] to `argv.iter().any(|a| a == SUBCOMMAND)` — the last two rows
    /// go red, and a user prompt merely CONTAINING the token would hijack the process into a pane.
    #[test]
    fn only_an_exact_argv_1_selects_the_inspector() {
        assert!(is_selected(&argv(&["cyrup", "__subagent-inspector"])));
        assert!(is_selected(&argv(&[
            "cyrup",
            "__subagent-inspector",
            "--async-dir",
            "/runs/r1",
            "--run-id",
            "r1",
        ])));
        assert!(!is_selected(&argv(&["cyrup"])));
        assert!(!is_selected(&argv(&[])));
        assert!(!is_selected(&argv(&["cyrup", "__subagent-runner"])));
        assert!(
            !is_selected(&argv(&["cyrup", "chat", "__subagent-inspector"])),
            "the token must be argv[1], not merely present"
        );
        assert!(
            !is_selected(&argv(&["cyrup", "tell me about __subagent-inspector"])),
            "a prompt mentioning the token is a prompt, not a subcommand"
        );
    }

    /// The token must be undiscoverable: absent from the user-facing subcommand surface, so
    /// `cyrup --help` cannot advertise a pane nobody can usefully open by hand.
    ///
    /// GUT: add `"__subagent-inspector"` to `subcommands::SUBCOMMANDS` — this goes red.
    #[test]
    fn the_inspector_subcommand_is_not_a_user_facing_subcommand() {
        // `first_subcommand` takes argv with the PROGRAM NAME ALREADY STRIPPED
        // (`subcommands.rs:46-47`), so the token under test is element 0 here. It is also the only
        // testable surface: `SUBCOMMANDS` itself is a private `const` (`subcommands.rs:36`).
        assert!(
            crate::subcommands::first_subcommand(&argv(&[SUBCOMMAND])).is_none(),
            "the internal token must not resolve as a user-facing subcommand"
        );
        assert!(
            crate::subcommands::first_subcommand(&argv(&[crate::subagent_runner_cmd::SUBCOMMAND]))
                .is_none(),
            "its sibling is undiscoverable for the same reason"
        );
    }
}
