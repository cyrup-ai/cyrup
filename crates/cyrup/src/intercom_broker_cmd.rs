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
//! Never advertised to users: not listed in `--help`, not one of [`crate::subcommands::SUBCOMMANDS`],
//! and dispatched from `main()` BEFORE any user-facing arg leniency/clap parsing runs — mirroring the
//! `__subagent-runner` pre-dispatch's placement.

/// The literal argv[0] token identifying this internal subcommand (mirrors
/// `cyrup_intercom::transport::spawn::INTERCOM_BROKER_SUBCOMMAND`).
pub const SUBCOMMAND: &str = "__intercom-broker";

/// Returns `true` if `argv` (the process's own args, *including* argv[0]/the binary name at index 0,
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn is_selected_matches_the_exact_internal_subcommand_token() {
        assert!(is_selected(&["cyrup".to_string(), "__intercom-broker".to_string()]));
        assert!(!is_selected(&["cyrup".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "__subagent-runner".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "--help".to_string()]));
        assert!(!is_selected(&[]));
    }
}
