//! Fixtures shared by more than one `lattice` submodule's tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::exec::acceptance::lattice::contract::VerifyCommand;
use crate::exec::acceptance::lattice::gate::CleanCompletionGate;
use crate::exec::completion_guard::CompletionMutationGuardResult;
use std::time::Duration;

/// A `verify[]` entry declaring nothing but its shell command — the run-level `cwd`, the
/// inherited environment and [`crate::exec::acceptance::model::DEFAULT_VERIFY_TIMEOUT_MS`] all apply.
pub(crate) fn vc(command: &str) -> VerifyCommand {
    VerifyCommand::shell(command)
}

/// The raw exit observation the retired `VerifyCommandResult.passed` field carried:
/// `exit_code == Some(0)`, verbatim (that field's own doc: "Whether this command counts as
/// passed: `exit_code == Some(0)`"). Every assertion below therefore keeps exactly the strength
/// it had before the two verify-result shapes collapsed onto upstream's single
/// [`crate::exec::acceptance::model::AcceptanceVerifyResult`]. NOTE it is deliberately NOT `status == Passed`: a command
/// declaring `allowFailure: true` that exits nonzero was `passed: false` with status
/// `allowed-failure`.
pub(crate) fn passed(result: &crate::exec::acceptance::model::AcceptanceVerifyResult) -> bool {
    result.exit_code == Some(0)
}

/// A `verify[]` entry declaring its own `timeoutMs`, for the timeout/kill-ladder tests that
/// must not wait out [`crate::exec::acceptance::model::DEFAULT_VERIFY_TIMEOUT_MS`].
pub(crate) fn vc_timeout(command: &str, timeout: Duration) -> VerifyCommand {
    VerifyCommand {
        timeout_ms: Some(
            u64::try_from(timeout.as_millis()).expect("a test timeout fits in u64 ms"),
        ),
        ..VerifyCommand::shell(command)
    }
}

// ---------------------------------------------------------------------------------------
// evaluate_acceptance: THE load-bearing DI-SA-5 distinction — a real executed verify[]
// command that exits 0 reaches Verified; a child merely CLAIMING success in prose does not.
// ---------------------------------------------------------------------------------------

pub(crate) fn clean_gate() -> CleanCompletionGate {
    CleanCompletionGate {
        exit_code: 0,
        detached: false,
        interrupted: false,
        timed_out: false,
    }
}

pub(crate) fn no_guard_trigger() -> CompletionMutationGuardResult {
    CompletionMutationGuardResult {
        expected_mutation: true,
        attempted_mutation: true,
        triggered: false,
    }
}
