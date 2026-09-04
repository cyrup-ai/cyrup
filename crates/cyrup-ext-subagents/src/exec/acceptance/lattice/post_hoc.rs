//! R-SA-033 post-hoc correction: the strictly-ordered exit-code and error rewrite applied when
//! the gate rejects an otherwise-clean run.

use super::gate::CleanCompletionGate;
use super::{AcceptanceLedger, AcceptanceStatus};

// ============================================================================================
// R-SA-033: post-hoc exit-code correction, strictly ordered
// ============================================================================================

/// The result of applying [`apply_post_hoc_correction`] — either the run's exit code/error are
/// left untouched, or both are rewritten per R-SA-033.
#[derive(Debug, Clone, PartialEq)]
pub struct PostHocCorrection {
    /// The exit code to report — unchanged from the input unless a correction was applied.
    pub exit_code: i32,
    /// The error text to report — unchanged from the input unless a correction was applied, in
    /// which case the rejection reason is appended to whatever error text (if any) was already
    /// present.
    pub error: Option<String>,
    /// Whether a correction was actually applied — exposed so a caller can log/assert on it
    /// separately from just re-deriving it by comparing `exit_code`s (which would be ambiguous
    /// if the input `exit_code` also happened to already be nonzero for an unrelated reason —
    /// though per R-SA-033's own gate that case never reaches this function to begin with, since
    /// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] itself already required `gate.is_clean()`, i.e. `exit_code == 0`,
    /// before ever producing a non-`NotRequired` ledger).
    pub corrected: bool,
}

/// A nonzero sentinel exit code this module uses when correcting an acceptance rejection
/// (R-SA-033: "overwrite the reported `exit_code` to a nonzero value"). `1` is the conventional
/// generic-failure exit code and carries no further encoded meaning — the actual reason always
/// lives in the accompanying error text, never in this numeric value alone.
pub const ACCEPTANCE_REJECTED_EXIT_CODE: i32 = 1;

/// R-SA-033 (MUST) — post-hoc exit-code correction, STRICTLY ORDERED to run after structured-
/// output validation (R-SA-030) and the completion-mutation guard (R-SA-034), and strictly before
/// the result is handed to the caller or serialized. This function itself does not enforce that
/// ordering procedurally (it cannot — it has no visibility into when its caller invokes it
/// relative to those other two checks); it is the caller's ([`crate::exec`]'s not-yet-built
/// `run_sync` completion path, a later phase) responsibility to invoke this LAST, after
/// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] itself, which is exactly why this function takes an already-computed
/// [`AcceptanceLedger`] as input rather than re-deriving one.
///
/// Correction is applied if AND ONLY IF all three hold:
/// 1. `ledger.status == AcceptanceStatus::Rejected` (the gate rejected the run);
/// 2. `contract_was_explicit` (R-SA-033: "AND the acceptance contract was explicit AND..." — a
///    heuristically-inferred contract's rejection is recorded on the ledger but never flips an
///    otherwise-successful `exit_code`, matching [`crate::exec::acceptance::lattice::contract::AcceptanceContract::explicit`]'s own doc
///    comment);
/// 3. the run otherwise succeeded per `gate.is_clean()` (restated here defensively rather than
///    trusted to the caller, exactly as [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] itself does — a correction applied
///    to an already-nonzero exit code would violate this function's own "a run that already
///    failed the completion guard is not corrected a second time" invariant, arch-SA §6.3.4).
///
/// When applied, `exit_code` becomes [`ACCEPTANCE_REJECTED_EXIT_CODE`] and the rejection reason
/// (`ledger.detail`, falling back to a generic message if absent) is appended to `existing_error`.
#[must_use]
pub fn apply_post_hoc_correction(
    ledger: &AcceptanceLedger,
    contract_was_explicit: bool,
    gate: CleanCompletionGate,
    existing_error: Option<&str>,
) -> PostHocCorrection {
    let should_correct =
        ledger.status == AcceptanceStatus::Rejected && contract_was_explicit && gate.is_clean();

    if !should_correct {
        return PostHocCorrection {
            exit_code: gate.exit_code,
            error: existing_error.map(str::to_string),
            corrected: false,
        };
    }

    let reason = ledger
        .detail
        .clone()
        .unwrap_or_else(|| "acceptance criteria were not met".to_string());
    let error = match existing_error {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}; acceptance rejected: {reason}")
        }
        _ => format!("acceptance rejected: {reason}"),
    };

    PostHocCorrection {
        exit_code: ACCEPTANCE_REJECTED_EXIT_CODE,
        error: Some(error),
        corrected: true,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::gate::evaluate_acceptance;
    use crate::exec::acceptance::lattice::lowering::lower_acceptance_input;
    use crate::exec::acceptance::lattice::testsupport::clean_gate;
    use crate::exec::acceptance::lattice::testsupport::no_guard_trigger;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_allow_failure_command_that_exits_nonzero_still_reaches_verified() {
        // The end-to-end shape from the bug report: an authored
        // `{ level: "verified", verify: [{ id: "lint", command: "…", allowFailure: true }] }`
        // whose command exits nonzero must still be ACCEPTED, so `apply_post_hoc_correction`
        // never rewrites the run's exit code to ACCEPTANCE_REJECTED_EXIT_CODE.
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [
                { "id": "tests", "command": "exit 0" },
                { "id": "lint", "command": "exit 1", "allowFailure": true },
            ],
        }))
        .expect("a well-formed acceptance policy must lower")
        .expect("an explicit contract");

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("Done.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Verified,
            "a verify[] command declaring allowFailure: true must not reject the run, got: \
             {ledger:?}"
        );
        assert_eq!(ledger.verify_results.len(), 2);
        assert_eq!(
            ledger.verify_results[1].status,
            crate::exec::acceptance::model::VerifyRunStatus::AllowedFailure
        );
        assert_eq!(
            apply_post_hoc_correction(&ledger, contract.explicit, clean_gate(), None).exit_code,
            0,
            "an accepted run must not have its exit code rewritten to \
             ACCEPTANCE_REJECTED_EXIT_CODE"
        );
    }

    // ---------------------------------------------------------------------------------------
    // apply_post_hoc_correction (R-SA-033)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn explicit_rejected_contract_on_a_clean_run_corrects_exit_code_and_appends_error() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("verify[] command failed".to_string()),
            verify_results: Vec::new(),
        };
        let correction =
            apply_post_hoc_correction(&ledger, true, clean_gate(), Some("prior warning"));
        assert!(correction.corrected);
        assert_eq!(correction.exit_code, ACCEPTANCE_REJECTED_EXIT_CODE);
        assert_ne!(correction.exit_code, 0);
        let error = correction.error.expect("error must be set");
        assert!(error.contains("prior warning"));
        assert!(error.contains("verify[] command failed"));
    }

    #[test]
    fn heuristic_inferred_rejected_contract_never_corrects_exit_code() {
        // R-SA-033: "AND the acceptance contract was explicit AND..." — a heuristic contract's
        // rejection must be recorded on the ledger but must NEVER flip an otherwise-0 exit code.
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("checked: guard triggered".to_string()),
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, false, clean_gate(), None);
        assert!(!correction.corrected);
        assert_eq!(correction.exit_code, 0);
        assert!(correction.error.is_none());
    }

    #[test]
    fn a_non_rejected_ledger_never_triggers_a_correction() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Verified,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Verified,
            detail: None,
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(!correction.corrected);
        assert_eq!(correction.exit_code, 0);
    }

    #[test]
    fn a_dirty_gate_never_triggers_a_correction_even_if_the_ledger_says_rejected() {
        // Defensive re-check: even a stale/inconsistent Rejected ledger must not correct a run
        // whose gate was not actually clean at the time this function runs (arch-SA §6.3.4: "a
        // run that already failed the completion guard is not corrected a second time").
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("stale".to_string()),
            verify_results: Vec::new(),
        };
        let dirty_gate = CleanCompletionGate {
            exit_code: 3,
            detached: false,
            interrupted: false,
            timed_out: false,
        };
        let correction = apply_post_hoc_correction(&ledger, true, dirty_gate, None);
        assert!(!correction.corrected);
        assert_eq!(
            correction.exit_code, 3,
            "the ORIGINAL nonzero exit code must be preserved unchanged, not overwritten"
        );
    }

    #[test]
    fn correction_with_no_prior_error_produces_a_clean_standalone_message() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("no verify[] commands were declared".to_string()),
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(correction.corrected);
        let error = correction.error.expect("error set");
        assert!(error.starts_with("acceptance rejected:"));
        assert!(
            !error.contains(';'),
            "no prior error to join with a separator"
        );
    }

    #[test]
    fn correction_falls_back_to_a_generic_reason_when_ledger_has_no_detail() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: None,
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(correction.corrected);
        assert!(
            correction
                .error
                .expect("error")
                .contains("acceptance criteria were not met")
        );
    }
}
