//! `__codemode-sandbox` — the hidden, never-user-facing CLI subcommand that is one codemode
//! script's sandbox process.
//!
//! The `codemode` tool runs each script's V8 isolate in a process of its own
//! ([`cyrup_codemode_runtime::tool::IsolatedSandboxFactory`]): it re-execs `current_exe()` (this
//! binary) with this subcommand, writes the script to the child's stdin and reads its messages from
//! the child's stdout. A script that exhausts memory, crashes the engine or ignores its deadline
//! then costs the session that one process and comes back to the model as a failed script. This
//! module recognizes the argv and hands to [`cyrup_codemode_runtime::sandbox::run_sandbox_process`],
//! which owns the protocol, the isolate and the child's resource limits.
//!
//! Dispatched from `main()` BEFORE anything that can write to stdout: stdout is the supervisor's
//! pipe, and the child's environment is empty, so nothing here may depend on settings or on the
//! agent directory either.
//!
//! # [CYRUP-DELTA] (SEAM-109) — an argv verb has no upstream counterpart at all
//!
//! **pi has NO argv verbs**; see [`crate::subagent_runner_cmd`]'s delta for the check that
//! establishes it. **The mechanism this replaces** is none: pi runs its script in a QuickJS VM
//! compiled to WebAssembly inside a worker thread (`packages/codemode/src/runtime/worker.ts`
//! @v1.0.4), where the WebAssembly boundary and the VM's own memory limit make a runaway script
//! a catchable error. V8 has neither: one allocation larger than the heap can satisfy is a fatal
//! out-of-memory that aborts the process, so the isolate lives in a disposable process instead
//! (ADR-0031, "a stronger posture ... is a later option"). cyrup is one compiled binary, so the
//! process is re-exec'd out of `current_exe()` under a reserved argv token, as the other hops are.
//!
//! **Deliberately undocumented:** `__`-prefixed, absent from `--help` and
//! `crate::subcommands::SUBCOMMANDS`, matched only as an exact `argv[1]` ([`is_selected`]).

use cyrup_codemode_runtime::sandbox::{HOST_SUBCOMMAND, run_sandbox_process};

/// The literal `argv[1]` token identifying this internal subcommand.
pub const SUBCOMMAND: &str = HOST_SUBCOMMAND;

/// Returns `true` if `argv` (the process's own args, *including* the binary name at index 0,
/// matching [`std::env::args`]'s shape) selects this internal subcommand.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.get(1).map(String::as_str) == Some(SUBCOMMAND)
}

/// Run the sandbox process to completion and return the exit code `main` should use.
///
/// The isolate builds a runtime of its own, which a tokio worker cannot do from inside a task, so
/// the work runs on a blocking thread.
pub async fn dispatch() -> i32 {
    tokio::task::spawn_blocking(run_sandbox_process)
        .await
        .unwrap_or(70)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn is_selected_matches_the_exact_internal_subcommand_token() {
        assert!(is_selected(&[
            "cyrup".to_string(),
            "__codemode-sandbox".to_string()
        ]));
        assert!(!is_selected(&["cyrup".to_string()]));
        assert!(!is_selected(&[
            "cyrup".to_string(),
            "__intercom-broker".to_string()
        ]));
        assert!(!is_selected(&[
            "cyrup".to_string(),
            "codemode-sandbox".to_string()
        ]));
        assert!(!is_selected(&[]));
    }
}
