//! R-SA-032 / DI-SA-5 gate evaluation: turning a contract plus observed evidence into a ledger
//! verdict.

use std::path::Path;

use crate::exec::completion_guard::CompletionMutationGuardResult;

use super::contract::{AcceptanceContract, ReviewerResult};
use super::report_source::{
    AcceptanceFileOutput, select_acceptance_report_source, self_report_floor,
};
use super::verify::run_verify_commands_memoized_with_cancel;
use super::{AcceptanceLedger, AcceptanceStatus};

// ============================================================================================
// R-SA-032: acceptance-gate evaluation
// ============================================================================================

/// The subset of a finished, otherwise-clean attempt's state [`evaluate_acceptance`] needs —
/// deliberately narrow (mirrors `exec/fallback.rs::AttemptSignal`'s own "only what the decision
/// actually branches on" convention) rather than depending on the not-yet-built `SingleResult`.
#[derive(Debug, Clone, Copy)]
pub struct CleanCompletionGate {
    pub exit_code: i32,
    pub detached: bool,
    pub interrupted: bool,
    pub timed_out: bool,
}

impl CleanCompletionGate {
    /// R-SA-033's own gate condition, restated as a predicate: `exit_code == 0 && !detached &&
    /// !interrupted && !timed_out`.
    #[must_use]
    pub fn is_clean(self) -> bool {
        self.exit_code == 0 && !self.detached && !self.interrupted && !self.timed_out
    }
}

/// R-SA-032 (MUST) / DI-SA-5 — evaluate the acceptance ledger for one finished attempt.
///
/// # Preconditions (defensively re-checked, never merely trusted to caller discipline)
///
/// `gate` MUST report [`CleanCompletionGate::is_clean`] — this function immediately returns a
/// [`AcceptanceLedger::not_required`]-shaped no-op (NOT an error: skipping evaluation on a
/// non-clean run is itself the correct, expected behavior R-SA-032's own text specifies — "after
/// a clean run... the orchestrator MUST evaluate") if the gate is not clean, regardless of what
/// `contract` demands. This mirrors R-SA-033's own explicit gate condition rather than requiring
/// every call site to remember to check it first.
///
/// # Evaluation ladder
///
/// 1. `contract.is_no_op()` → [`AcceptanceStatus::NotRequired`], no further work.
/// 2. Otherwise, extract the child's own `acceptance-report` block (if any) from `final_output`
///    via `extract_acceptance_report` — its PRESENCE (not its claimed truthiness) establishes
///    at least [`AcceptanceStatus::Claimed`]; if the extracted JSON additionally carries at least
///    one of [`crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS`], the floor rises to
///    [`AcceptanceStatus::Attested`] (still self-report, richer self-report).
/// 3. If `contract.required_level >= Checked`, a structural/evidence check runs:
///    `completion_guard.triggered == false` (i.e. the completion-mutation guard did NOT fire) is
///    the evidence this module consults — an orchestrator-observed fact, not a child assertion —
///    and raising the achieved level to [`AcceptanceStatus::Checked`] when it holds.
/// 4. If `contract.required_level >= Verified`, [`crate::exec::acceptance::lattice::verify::run_verify_commands`] is ACTUALLY invoked
///    against every declared command; achieving [`AcceptanceStatus::Verified`] requires that **no**
///    result [`rejects`](crate::exec::acceptance::model::AcceptanceVerifyResult::rejects) — a single failing (or timed-out)
///    command caps the achieved level below `Verified` regardless of how many others passed, and
///    regardless of anything the child's own report claims. A command that declared
///    `allowFailure: true` and merely exited nonzero is [`crate::exec::acceptance::model::VerifyRunStatus::AllowedFailure`]
///    and does NOT cap the level — upstream `evaluateAcceptance` rejects only on
///    `status === "failed" || status === "timed-out"` (`acceptance.ts:1297` @v0.43.0).
/// 5. If `contract.required_level >= Reviewed`, `contract.reviewer_result` MUST be
///    `Some(ReviewerResult { approved: true, .. })` to reach [`AcceptanceStatus::Reviewed`]; a
///    `None` reviewer result or an `approved: false` one caps the achieved level below `Reviewed`.
/// 6. If the achieved level (after steps 2-5) does not satisfy `contract.required_level`
///    ([`AcceptanceStatus::satisfies`]), the final status is [`AcceptanceStatus::Rejected`] with a
///    `detail` explaining which required level was not reached; otherwise the achieved level
///    itself is the final status.
///
/// `verify_results` on the returned ledger is populated iff step 4 actually ran (i.e.
/// `contract.required_level >= Verified`); empty otherwise.
///
/// # Relationship to [`crate::exec::acceptance::model::evaluate_acceptance`]
///
/// Upstream has ONE gate. This is the one `exec/mod.rs::run_sync` calls for every ordinary run;
/// [`crate::exec::acceptance::model::evaluate_acceptance`] is the faithful port of the same upstream function, and its only
/// production caller is `spawn::chain_graph`'s dynamic-group gate. That split is unfinished port
/// work — see the header comment above [`crate::exec::acceptance::model`] for exactly which layers have already collapsed
/// onto the [`crate::exec::acceptance::model`] side and which have not.
///
/// What matters for a reader of the RETURNED ledger: every piece of evidence upstream records on a
/// verify run reaches this ledger, because [`AcceptanceLedger::verify_results`] holds upstream's
/// own [`crate::exec::acceptance::model::AcceptanceVerifyResult`], and [`AcceptanceLedger::evidence_status`] is upstream's
/// own `evidenceStatus`. What does NOT reach it is upstream's structured `runtimeChecks` /
/// `childReport` / `reviewResult`: this function still flattens its findings into `detail`.
#[must_use]
pub async fn evaluate_acceptance(
    contract: &AcceptanceContract,
    gate: CleanCompletionGate,
    final_output: Option<&str>,
    completion_guard: CompletionMutationGuardResult,
    verify_cwd: &Path,
    // G80 — pi's `artifactsDir`/`runId` pair (`acceptance.ts:1226-1227`), threaded down to
    // `runMemoizedVerifyCommand`. `None` (every caller with no artifacts root configured, and
    // every test that does not exercise memoization) executes every verify[] command for real.
    memo: Option<crate::exec::acceptance::model::VerifyMemoContext<'_>>,
    // G82 — pi's `fileOutput` (`acceptance.ts:1214-1220`): content the CHILD itself sent to its
    // configured output file, taken from its own successful `write` tool calls rather than from
    // disk, so a concurrent writer to the same path cannot be misattributed to it. `None` for a
    // run with no configured output path, or one whose child never successfully wrote it.
    file_output: Option<AcceptanceFileOutput<'_>>,
) -> AcceptanceLedger {
    // pi's `signal` is OPTIONAL (`acceptance.ts:1073`), and this entry is its `signal: undefined`
    // form — the shape every caller that has no cancellation to offer (the whole test surface, and
    // the group gate, which upstream likewise calls without a signal) uses. Production runs go
    // through [`evaluate_acceptance_with_cancel`].
    evaluate_acceptance_with_cancel(
        contract,
        gate,
        final_output,
        completion_guard,
        verify_cwd,
        memo,
        file_output,
        &cyrup_core::CancelToken::new(),
    )
    .await
}

/// SUBA-028 — [`evaluate_acceptance`] with the run's cancellation token (pi's `signal?:
/// AbortSignal`, `acceptance.ts:1073`, threaded to `runMemoizedVerifyCommand` at `:1290` and
/// checked between commands at `:1295`).
///
/// # The gap this closes
///
/// Cancelling a subagent run — Ctrl-C, an orchestrator cancel, a parent timeout — did not reach
/// acceptance verification at all: the caller could be made to wait a full per-command
/// `timeoutMs` (default minutes) after asking to stop, once per remaining command. SUBA-027's fix
/// means the timed-out CHILD is killed, so nothing leaks; the LATENCY was what remained.
///
/// The token races the verify child's own `wait()` inside
/// [`crate::exec::acceptance::model::run_verify_command_with_cancel`], so a cancellation lands mid-command rather than only
/// between commands, and the abort path is upstream's `abortVerification` — SIGTERM, hard SIGKILL
/// a second later, targeting the command's process GROUP.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_acceptance_with_cancel(
    contract: &AcceptanceContract,
    gate: CleanCompletionGate,
    final_output: Option<&str>,
    completion_guard: CompletionMutationGuardResult,
    verify_cwd: &Path,
    memo: Option<crate::exec::acceptance::model::VerifyMemoContext<'_>>,
    file_output: Option<AcceptanceFileOutput<'_>>,
    cancel: &cyrup_core::CancelToken,
) -> AcceptanceLedger {
    if !gate.is_clean() {
        return AcceptanceLedger::not_required();
    }
    if contract.is_no_op() {
        return AcceptanceLedger::not_required();
    }

    // G82 / pi `parseAcceptanceReportSources` (`acceptance.ts:753-771`): the acceptance report may
    // live in the assistant's own output OR in the artifact the child wrote to its configured
    // output path, and in `outputMode: "file-only"` the FILE is searched first. Resolved once here
    // and used by every rung below that reads the report.
    let report_source = select_acceptance_report_source(final_output, file_output.as_ref());

    // Step 2: self-report floor (Claimed / Attested).
    let mut achieved = self_report_floor(report_source);

    // Step 3: Checked — orchestrator-observed structural evidence.
    let mut detail: Vec<String> = Vec::new();
    if contract.required_level >= AcceptanceStatus::Checked {
        let mut checked = true;
        if completion_guard.triggered {
            detail.push(
                "checked: completion-mutation guard triggered (implementation was expected but \
                 no mutating tool call was observed)"
                    .to_string(),
            );
            checked = false;
        }
        // pi's own `rank >= checked` rung (`evaluateAcceptance`, `acceptance.ts:1297-1321`): every
        // declared criterion must appear in the child's report as `satisfied`, every declared
        // evidence kind must be present in it, and `no-staged-files` additionally shells out to a
        // REAL `git status --short`. ANY failed runtime check rejects. Runs alongside — not instead
        // of — the completion-mutation guard above, which is this crate's own extra orchestrator-
        // observed signal (R-SA-034) and has no upstream counterpart on this rung.
        let failures = declared_structural_failures(contract, report_source, verify_cwd).await;
        if !failures.is_empty() {
            detail.extend(failures);
            checked = false;
        }
        if checked {
            achieved = achieved.max(AcceptanceStatus::Checked);
        }
    }

    // Step 4: Verified — REAL verify[] execution (DI-SA-5's whole reason for being).
    let mut verify_results = Vec::new();
    if contract.required_level >= AcceptanceStatus::Verified {
        if contract.verify.is_empty() {
            detail.push("verified: no verify[] commands were declared".to_string());
        } else {
            verify_results = run_verify_commands_memoized_with_cancel(
                &contract.verify,
                verify_cwd,
                memo,
                cancel,
            )
            .await;
            // `verifyRuns.some((run) => run.status === "failed" || run.status === "timed-out")`
            // (`acceptance.ts:1297` @v0.43.0) — NOT `!every(passed)`, which would also reject a
            // command the author explicitly marked `allowFailure: true`.
            let failed: Vec<&str> = verify_results
                .iter()
                .filter(|r| r.rejects())
                .map(|r| r.command.as_str())
                .collect();
            if failed.is_empty() {
                achieved = achieved.max(AcceptanceStatus::Verified);
            } else {
                detail.push(format!(
                    "verified: {} of {} verify[] command(s) failed: {}",
                    failed.len(),
                    verify_results.len(),
                    failed.join(", ")
                ));
            }
        }
    }

    // G78 — upstream's `evidenceStatus` is frozen HERE, before the review rung: `evaluateAcceptance`
    // writes both fields in lockstep through the attestation/checked/verified rungs
    // (`acceptance.ts:1254-1255,1275-1276,1303-1304`) and its review block (`:1318-1336`) then
    // rewrites `status` ALONE. Snapshotting `achieved` at this exact point is that same freeze:
    // everything above is evidence the orchestrator observed, everything below is review.
    let evidence_achieved = achieved;

    // Step 5: Reviewed — externally-supplied reviewer result only.
    if contract.required_level >= AcceptanceStatus::Reviewed {
        match &contract.reviewer_result {
            Some(ReviewerResult { approved: true, .. }) => {
                achieved = achieved.max(AcceptanceStatus::Reviewed);
            }
            Some(ReviewerResult {
                approved: false,
                detail: reviewer_detail,
            }) => {
                detail.push(format!(
                    "reviewed: independent reviewer did not approve{}",
                    reviewer_detail
                        .as_deref()
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default()
                ));
            }
            // pi `evaluateAcceptance`'s `optionalReview` branch (`acceptance.ts:1321-1335`): a
            // declared `review: { required: false }` gate makes a missing reviewer result a
            // NON-BLOCKING finding rather than a rejection, so the run keeps whatever level it
            // otherwise reached. Any other shape (a required gate, `review: false`, or no `review`
            // key at all) still caps the level below `Reviewed`.
            None if matches!(
                &contract.review,
                Some(crate::exec::acceptance::model::ReviewSetting::Gate(gate)) if gate.required == Some(false)
            ) =>
            {
                achieved = achieved.max(AcceptanceStatus::Reviewed);
            }
            None => {
                detail.push("reviewed: no independent reviewer result was supplied".to_string());
            }
        }
    }

    // Step 6: final verdict.
    if achieved.satisfies(contract.required_level) {
        AcceptanceLedger {
            status: achieved,
            evidence_status: evidence_achieved.evidence_status(),
            detail: if detail.is_empty() {
                None
            } else {
                Some(detail.join("; "))
            },
            verify_results,
        }
    } else {
        let reason = if detail.is_empty() {
            format!(
                "required {} but only reached {}",
                contract.required_level.as_wire_str(),
                achieved.as_wire_str()
            )
        } else {
            detail.join("; ")
        };
        AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            // Every rejecting arm upstream writes `rejected` to BOTH fields
            // (`acceptance.ts:1260-1261,1283-1284,1298-1299,1309-1310`) — a rejection is not an
            // evidence level that was earned and then withheld pending review, so unlike the review
            // rung it does move `evidenceStatus`.
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some(reason),
            verify_results,
        }
    }
}

/// Step 3's declared-policy half: pi's `checkCriteriaSatisfied` + `runStructuralChecks`
/// (`acceptance.ts:911-966` @v0.43.0, applied on `evaluateAcceptance`'s `rank >= checked` rung at
/// `:819-829`), run against the child's own `acceptance-report` block.
///
/// Returns one message per FAILED [`crate::exec::acceptance::model::AcceptanceRuntimeCheck`], verbatim, so the ledger detail
/// (and therefore [`crate::exec::acceptance::lattice::post_hoc::apply_post_hoc_correction`]'s error text) carries pi's own wording —
/// e.g. `tests-added evidence missing from child report.` or
/// `Required criterion 'c1' was not reported.`. An empty vector means every declared check passed,
/// which is also the trivial answer when the contract declares no criteria and no evidence.
///
/// The child's report is parsed by [`crate::exec::acceptance::model::parse_acceptance_report`] — the same faithful port
/// `exec/mod.rs` already uses to STRIP the block from the delivered output — rather than by this
/// module's own `extract_acceptance_report`, which is a presence/companion-key probe for the
/// `Claimed` vs `Attested` floor and cannot answer "is criterion `c1` reported satisfied". A child
/// that emitted no parseable report at all is treated as an empty report, so every declared check
/// fails with pi's "missing"/"not reported" message rather than being skipped.
async fn declared_structural_failures(
    contract: &AcceptanceContract,
    final_output: Option<&str>,
    cwd: &Path,
) -> Vec<String> {
    if contract.criteria.is_empty() && contract.evidence.is_empty() {
        return Vec::new();
    }
    let report = final_output
        .map(crate::exec::acceptance::model::parse_acceptance_report)
        .and_then(|parsed| parsed.report)
        .unwrap_or_default();
    let mut checks =
        crate::exec::acceptance::model::check_criteria_satisfied(&contract.criteria, &report);
    checks.extend(
        crate::exec::acceptance::model::run_structural_checks(&contract.evidence, &report, cwd)
            .await,
    );
    checks
        .into_iter()
        .filter(|check| check.status == crate::exec::acceptance::model::RuntimeCheckStatus::Failed)
        .map(|check| check.message)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::testsupport::clean_gate;
    use crate::exec::acceptance::lattice::testsupport::no_guard_trigger;
    use crate::exec::acceptance::lattice::testsupport::passed;
    use crate::exec::acceptance::lattice::testsupport::vc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_executed_passing_verify_command_reaches_verified() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 0")]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("I fixed the bug.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Verified,
            "a REAL verify[] command that actually exits 0 must reach Verified, got: {ledger:?}"
        );
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(passed(&ledger.verify_results[0]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_childs_prose_claim_of_success_alone_never_reaches_verified() {
        // THE required scenario from the task brief: a child that merely claims success in prose
        // (no verify[] commands declared at all, or verify[] commands declared but the child's
        // own prose asserting they "passed" is irrelevant — this module never even reads that
        // assertion) must NOT reach Verified when Verified is the required level and no REAL
        // verify[] evidence exists.
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![]); // nothing declared to actually run

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(
                "I ran the tests and they passed.\n```acceptance-report\n\
                 {\"criteriaSatisfied\": true, \"commandsRun\": [\"cargo test\"]}\n```",
            ),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Rejected,
            "a child's own prose claim of success, with NO actually-executed verify[] command, \
             must NOT satisfy a Verified requirement — got: {ledger:?}"
        );
        assert!(ledger.verify_results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_executed_failing_verify_command_is_rejected_regardless_of_prose_claim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 1")]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("Everything passed!\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(!passed(&ledger.verify_results[0]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_failing_command_among_several_caps_below_verified_even_if_others_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(
            AcceptanceStatus::Verified,
            vec![vc("exit 0"), vc("exit 1"), vc("exit 0")],
        );
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn not_clean_gate_short_circuits_to_not_required_regardless_of_contract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 0")]);
        let dirty_gate = CleanCompletionGate {
            exit_code: 1,
            detached: false,
            interrupted: false,
            timed_out: false,
        };
        let ledger = evaluate_acceptance(
            &contract,
            dirty_gate,
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::NotRequired);
        assert!(
            ledger.verify_results.is_empty(),
            "must not even run verify[] on a non-clean gate"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_level_is_satisfied_by_a_non_triggered_completion_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Checked);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_level_is_rejected_when_completion_guard_triggered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let triggered = CompletionMutationGuardResult {
            expected_mutation: true,
            attempted_mutation: false,
            triggered: true,
        };
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            triggered,
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_requires_an_approved_externally_supplied_reviewer_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: true,
                detail: Some("looks good".to_string()),
            });
        assert_eq!(contract.required_level, AcceptanceStatus::Reviewed);
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Reviewed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_rejected_when_reviewer_result_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        contract.required_level = AcceptanceStatus::Reviewed; // demand Reviewed but attach no result
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_rejected_when_reviewer_disapproved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: false,
                detail: Some("needs more work".to_string()),
            });
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert!(ledger.detail.expect("detail").contains("needs more work"));
    }

    /// G79 on the LIVE gate: `run_sync` calls THIS `evaluate_acceptance`, whose `Checked` rung
    /// parses the child's report through `crate::exec::acceptance::model::parse_acceptance_report`. A child that answered in
    /// snake_case, wrote `Done` instead of `satisfied`, sent a lone object where an array belongs
    /// and a bare string where a `string[]` belongs used to fail every declared criterion and every
    /// declared evidence kind — a rejected run for a purely cosmetic mismatch. After G79 the same
    /// answer passes, and the criterion id matches across `c 1` / `C_1` spellings.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_gate_accepts_an_aliased_child_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]).with_policy(
            crate::exec::acceptance::model::normalize_criteria(
                &[crate::exec::acceptance::model::CriterionInput::Gate(
                    crate::exec::acceptance::model::AcceptanceGate {
                        id: Some("C 1".to_string()),
                        must: Some("add a regression test".to_string()),
                        evidence: None,
                        severity: None,
                    },
                )],
                &[],
            ),
            vec![
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ChangedFiles,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::TestsAdded,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::CommandsRun,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ManualNotes,
            ],
            None,
            Vec::new(),
        );
        let output = r#"done
```acceptance_report
{
  "criteria_satisfied": {"id": "c_1", "status": "Done", "evidence": "added tests/regression.rs"},
  "changed_files": "src/file.rs",
  "tests_added_or_updated": ["tests/regression.rs"],
  "commands_run": {"command": "cargo test", "result": "OK", "summary": "green"},
  "manual_notes": "nothing else"
}
```"#;
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(output),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(
            ledger.status,
            AcceptanceStatus::Checked,
            "aliased report must satisfy the gate; detail was {:?}",
            ledger.detail
        );
        assert_eq!(ledger.detail, None);
    }
}
