//! `evaluateAcceptance`/`acceptanceFailureMessage` (pi `acceptance.ts:769-856`): the gate that
//! turns criteria, evidence and verify runs into a ledger.

use std::path::Path;

use serde_json::Value;

use super::checks::{check_criteria_satisfied, run_structural_checks};
use super::prompt::acceptance_requires_child_report;
use super::report::parse::{parse_acceptance_report_sources, ACCEPTANCE_REPORT_NOT_FOUND, ParsedAcceptanceReport};
use super::report::validate::validate_acceptance_report;
use super::types::{level_rank, AcceptanceEvidenceStatus, AcceptanceLedger, AcceptanceLedgerStatus, AcceptanceLevel, AcceptanceReport, AcceptanceReviewResult, AcceptanceRuntimeCheck, ResolvedAcceptanceConfig, ReviewFinding, ReviewFindingSeverity, ReviewResultStatus, ReviewSetting, RuntimeCheckStatus, SerializableGate, VerifyRunStatus};
use super::verify::memo::{run_memoized_verify_command, VerifyMemoContext};

// --------------------------------------------------------------------------------------------
// evaluateAcceptance / acceptanceFailureMessage (acceptance.ts:769-856)
// --------------------------------------------------------------------------------------------

/// Input to [`evaluate_acceptance`] (`acceptance.ts:1210-1228`).
pub struct EvaluateAcceptanceInput<'a> {
    pub acceptance: &'a ResolvedAcceptanceConfig,
    pub output: &'a str,
    pub cwd: &'a Path,
    pub report: Option<AcceptanceReport>,
    /// G79 — pi `input.fileOutput` (`acceptance.ts:1214-1220`): the content the child sent to
    /// its configured output file, searched for the acceptance report alongside `output` by
    /// [`parse_acceptance_report_sources`].
    pub file_output: Option<crate::exec::acceptance::AcceptanceFileOutput<'a>>,
    pub review_result: Option<AcceptanceReviewResult>,
    /// G80 — pi `input.artifactsDir` + `input.runId` (`acceptance.ts:1226-1227`), threaded to
    /// [`run_memoized_verify_command`] (`acceptance.ts:1289-1293`). `None` disables
    /// memoization for this evaluation, which is what pi's own chain-execution group gate does
    /// (`chain-execution.ts:1037-1046,1233-1242` pass neither field).
    pub memo: Option<VerifyMemoContext<'a>>,
    /// G78 — pi `input.reportOptional` (`acceptance.ts:1225`): this caller does not insist the
    /// child emit a structured `acceptance-report` block, because it did not ASK for one.
    ///
    /// It is the same flag that suppresses the contract prompt
    /// (`formatAcceptancePrompt(acceptance, { reportOptional })` returns `""` when the policy
    /// declares neither criteria nor evidence, `acceptance.ts:409`), so leaving it `false` here
    /// while the prompt was suppressed would reject every such run for missing a report it was
    /// never told to write. See [`acceptance_requires_child_report`] and the three-arm ladder
    /// at `acceptance.ts:1251-1266`.
    pub report_optional: bool,
}

/// `evaluateAcceptance` (acceptance.ts:1210-1355). Async because `verified` runs REAL `verify[]`
/// subprocesses and `checked`/above may run the REAL `git status` no-staged-files check.
#[must_use]
pub async fn evaluate_acceptance(input: EvaluateAcceptanceInput<'_>) -> AcceptanceLedger {
    let acceptance = input.acceptance;
    // `acceptance.ts:1230-1233` @v0.43.0: ONE `initialStatus`, written to both fields.
    let initial_status = if acceptance.level == AcceptanceLevel::None {
        AcceptanceEvidenceStatus::NotRequired
    } else {
        AcceptanceEvidenceStatus::Claimed
    };
    let mut ledger = AcceptanceLedger {
        status: initial_status.into(),
        evidence_status: initial_status,
        explicit: acceptance.explicit,
        inferred_reason: acceptance.inferred_reason.clone(),
        criteria: acceptance.criteria.iter().map(SerializableGate::from_gate).collect(),
        child_report: Option::None,
        child_report_parse_error: Option::None,
        runtime_checks: Vec::new(),
        verify_runs: Vec::new(),
        review_result: Option::None,
    };
    if acceptance.level == AcceptanceLevel::None {
        return ledger;
    }

    // `acceptance.ts:1243-1250` @v0.43.0 — a caller-supplied `report` wins outright, but is
    // RE-VALIDATED first (v0.43.0 wrapped it in `validateAcceptanceReport`; before that it was
    // trusted verbatim). Otherwise BOTH the assistant output and the child's configured output
    // file are searched, in the order `parse_acceptance_report_sources` picks (G79).
    let parsed = match input.report {
        Some(supplied) => {
            // `validateAcceptanceReport(input.report)` — upstream's default `pathLabel` is `""`
            // (`acceptance.ts:831`), which is also what every other caller in this port passes.
            let value = serde_json::to_value(&supplied).unwrap_or(Value::Null);
            let (validated, errors) = validate_acceptance_report(&value, "");
            match validated {
                Some(report) => ParsedAcceptanceReport {
                    report: Some(report),
                    error: Option::None,
                },
                Option::None => ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(format!(
                        "Failed to parse acceptance-report: Invalid acceptance-report: {}",
                        errors.join("; ")
                    )),
                },
            }
        }
        Option::None => parse_acceptance_report_sources(input.output, input.file_output.as_ref()),
    };
    // `const needsReport = acceptanceRequiresChildReport(acceptance)` (`acceptance.ts:1251`).
    let needs_report = acceptance_requires_child_report(acceptance);
    // The three-arm ladder at `acceptance.ts:1252-1266` @v0.43.0. Before this port it had only
    // two arms — report present, or reject — so a `report_optional` caller (one whose contract
    // prompt was itself suppressed, `acceptance.ts:409`) was rejected for failing to emit a
    // report it was never asked for.
    let report = parsed.report.clone();
    if let Some(report) = &report {
        ledger.child_report = Some(report.clone());
        ledger.status = AcceptanceLedgerStatus::Attested;
        ledger.evidence_status = AcceptanceEvidenceStatus::Attested;
    } else if !input.report_optional
        || needs_report
        || parsed.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND)
    {
        // A missing report still RECORDS a failed attestation check when the caller demanded
        // one, when the policy's own criteria/evidence require one, or when the report was
        // present but BROKEN (any error other than "not found").
        ledger.child_report_parse_error = parsed.error.clone();
        ledger.runtime_checks.push(AcceptanceRuntimeCheck {
            id: "attestation".to_string(),
            status: RuntimeCheckStatus::Failed,
            message: parsed
                .error
                .clone()
                .unwrap_or_else(|| "Structured acceptance report missing.".to_string()),
        });
        if !input.report_optional {
            ledger.status = AcceptanceLedgerStatus::Rejected;
            ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
            return ledger;
        }
    } else {
        // `acceptance.ts:1264-1266` — the non-rejecting arm: an OPTIONAL report that is simply
        // absent from a policy that requires none. The parse error is recorded for the
        // transcript and NO runtime check is pushed, so the run keeps climbing and settles at
        // its declared level via the `claimed` fallthrough below (`:1313-1316`).
        ledger.child_report_parse_error = parsed.error.clone();
    }

    let rank = level_rank(acceptance.level).unwrap_or(0);
    let checked_rank = 2u8;
    let verified_rank = 3u8;

    // `acceptance.ts:1268-1278` @v0.43.0. NOTE the three structural changes v0.43.0 made here:
    // the rung is additionally guarded on `parsed.report` (there is nothing to check criteria
    // against without one, now that the ladder above can fall through with none), it APPENDS to
    // `runtimeChecks` rather than replacing them, and it no longer returns early on a failed
    // check — it simply declines to promote, so the `verify[]` rung below still runs and its
    // results still land on the ledger. The single rejection point is the combined check
    // further down (`:1308-1312`).
    if let Some(report) = &report
        && rank >= checked_rank
    {
        let mut checks = check_criteria_satisfied(&acceptance.criteria, report);
        checks.extend(run_structural_checks(&acceptance.evidence, report, input.cwd).await);
        ledger.runtime_checks.extend(checks);
        if !ledger
            .runtime_checks
            .iter()
            .any(|c| c.status == RuntimeCheckStatus::Failed)
        {
            ledger.status = AcceptanceLedgerStatus::Checked;
            ledger.evidence_status = AcceptanceEvidenceStatus::Checked;
        }
    }

    if rank >= verified_rank
        && (acceptance.level == AcceptanceLevel::Verified || !acceptance.verify.is_empty())
    {
        if acceptance.level == AcceptanceLevel::Verified && acceptance.verify.is_empty() {
            ledger.runtime_checks.push(AcceptanceRuntimeCheck {
                id: "verification-config".to_string(),
                status: RuntimeCheckStatus::Failed,
                message: "verified acceptance requires runtime verify commands.".to_string(),
            });
            ledger.status = AcceptanceLedgerStatus::Rejected;
            ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
            return ledger;
        }
        let mut runs = Vec::new();
        for command in &acceptance.verify {
            // `runMemoizedVerifyCommand(command, input.cwd, { …, artifactsDir, runId })`
            // (`acceptance.ts:1289-1293`) — memoized when the caller supplied both, a plain
            // execution otherwise.
            //
            // SUBA-028: no `signal`/`abortMessage` is threaded here, and that is upstream's
            // own shape for THIS entry's only production caller — `spawn::chain_graph`'s
            // completed-GROUP gate, whose two upstream counterparts
            // (`chain-execution.ts:1037-1046,1233-1242` @v0.43.0) pass neither. The per-RUN
            // gate, which upstream does give a signal, is the sibling
            // [`crate::exec::acceptance::evaluate_acceptance_with_cancel`]. If a caller with a live token ever
            // reaches this entry, thread it through `run_memoized_verify_command_with_cancel`
            // and add the `if aborted break` after the push (`acceptance.ts:1295`) rather than
            // leaving the token unread here.
            runs.push(run_memoized_verify_command(command, input.cwd, input.memo).await);
        }
        ledger.verify_runs = runs;
        if ledger.verify_runs.iter().any(|run| {
            matches!(run.status, VerifyRunStatus::Failed | VerifyRunStatus::TimedOut)
        }) {
            ledger.status = AcceptanceLedgerStatus::Rejected;
            ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
            return ledger;
        }
        if !ledger
            .runtime_checks
            .iter()
            .any(|c| c.status == RuntimeCheckStatus::Failed)
        {
            ledger.status = AcceptanceLedgerStatus::Verified;
            ledger.evidence_status = AcceptanceEvidenceStatus::Verified;
        }
    }

    // `acceptance.ts:1308-1312` — the single rejection point for failed structural checks.
    if ledger
        .runtime_checks
        .iter()
        .any(|c| c.status == RuntimeCheckStatus::Failed)
    {
        ledger.status = AcceptanceLedgerStatus::Rejected;
        ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
        return ledger;
    }
    // `acceptance.ts:1313-1316` — a run that never got past `claimed` (only reachable when the
    // caller allowed a missing report) still settles at its declared level.
    if ledger.status == AcceptanceLedgerStatus::Claimed {
        let settled = match acceptance.level {
            AcceptanceLevel::Verified => AcceptanceEvidenceStatus::Verified,
            AcceptanceLevel::Checked => AcceptanceEvidenceStatus::Checked,
            AcceptanceLevel::Attested => AcceptanceEvidenceStatus::Attested,
            AcceptanceLevel::None => AcceptanceEvidenceStatus::NotRequired,
            // `auto` never survives `resolve_effective_acceptance`.
            AcceptanceLevel::Auto => AcceptanceEvidenceStatus::Claimed,
        };
        ledger.status = settled.into();
        ledger.evidence_status = settled;
    }

    // `acceptance.ts:1318-1336` @v0.43.0 — the review gate now hangs off `acceptance.review`,
    // NOT off a `level === "reviewed"` that no longer exists. Only `status` moves here;
    // `evidence_status` keeps whatever the evidence actually earned.
    if let Some(ReviewSetting::Gate(gate)) = &acceptance.review {
        match input.review_result {
            Some(review) if review.status == ReviewResultStatus::Reviewed => {
                ledger.review_result = Some(review);
                ledger.status = AcceptanceLedgerStatus::Reviewed;
            }
            Some(review) if review.status == ReviewResultStatus::Blockers => {
                ledger.review_result = Some(review);
                ledger.status = AcceptanceLedgerStatus::Rejected;
            }
            supplied => {
                if gate.required != Some(false) {
                    ledger.review_result = Some(supplied.unwrap_or(AcceptanceReviewResult {
                        status: ReviewResultStatus::ReviewRequired,
                        findings: vec![ReviewFinding {
                            severity: ReviewFindingSeverity::NonBlocking,
                            file: Option::None,
                            issue: "Independent review has not been supplied.".to_string(),
                            rationale:
                                "The run cannot be marked reviewed from child evidence alone."
                                    .to_string(),
                        }],
                    }));
                    ledger.status = AcceptanceLedgerStatus::ReviewRequired;
                }
            }
        }
    }

    ledger
}

/// `acceptanceFailureMessage` (acceptance.ts:1357-1365).
#[must_use]
pub fn acceptance_failure_message(ledger: &AcceptanceLedger) -> Option<String> {
    if ledger.status != AcceptanceLedgerStatus::Rejected {
        return Option::None;
    }
    if let Some(check) = ledger
        .runtime_checks
        .iter()
        .find(|c| c.status == RuntimeCheckStatus::Failed)
    {
        return Some(format!("Acceptance rejected: {}", check.message));
    }
    if let Some(run) = ledger
        .verify_runs
        .iter()
        .find(|r| matches!(r.status, VerifyRunStatus::Failed | VerifyRunStatus::TimedOut))
    {
        let status = match run.status {
            VerifyRunStatus::Failed => "failed",
            VerifyRunStatus::TimedOut => "timed-out",
            _ => "failed",
        };
        return Some(format!("Acceptance verification '{}' {status}.", run.id));
    }
    // `acceptance.ts:1363-1364` @v0.43.0. v0.34.0 also had a `needs-parent-decision` arm; that
    // review status no longer exists, and its successor (`review-required`) is not a REJECTED
    // ledger, so it never reaches this function at all.
    match ledger.review_result.as_ref().map(|r| r.status) {
        Some(ReviewResultStatus::Blockers) => {
            Some("Acceptance review found blockers.".to_string())
        }
        _ => Some("Acceptance rejected.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::aggregate::aggregate_acceptance_report;
    use crate::exec::acceptance::model::level::AcceptanceResolveInput;
    use crate::exec::acceptance::model::report::parse::parse_acceptance_report;
    use crate::exec::acceptance::model::testsupport::attested_policy_requiring_no_report;
    use crate::exec::acceptance::model::testsupport::cfg;
    use crate::exec::acceptance::model::testsupport::report_text;
    use crate::exec::acceptance::model::testsupport::resolve;
    use crate::exec::acceptance::model::testsupport::temp_dir;
    use crate::exec::acceptance::model::types::AcceptanceConfig;
    use crate::exec::acceptance::model::types::AcceptanceEvidenceKind;
    use crate::exec::acceptance::model::types::AcceptanceGate;
    use crate::exec::acceptance::model::types::AcceptanceReviewGate;
    use crate::exec::acceptance::model::types::CriterionInput;
    use serde_json::json;


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_optional_does_not_reject_a_merely_absent_report() {
        // `acceptance.ts:1264-1266` — the third arm: no runtime check is pushed and the run
        // keeps climbing, settling at its declared level via `:1313-1316`.
        let dir = temp_dir();
        let acceptance = attested_policy_requiring_no_report();
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "just prose, no report block",
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: true,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Attested);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Attested);
        assert!(
            ledger.runtime_checks.is_empty(),
            "no attestation check may be recorded on this arm: {:?}",
            ledger.runtime_checks
        );
        assert_eq!(
            ledger.child_report_parse_error.as_deref(),
            Some(ACCEPTANCE_REPORT_NOT_FOUND),
            "the absence is still RECORDED for the transcript"
        );
        assert!(acceptance_failure_message(&ledger).is_none());
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_same_absent_report_still_rejects_when_the_caller_is_not_report_optional() {
        // The control for the test above: only `report_optional` changes.
        let dir = temp_dir();
        let acceptance = attested_policy_requiring_no_report();
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "just prose, no report block",
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
        assert_eq!(ledger.runtime_checks.len(), 1);
        assert_eq!(ledger.runtime_checks[0].id, "attestation");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_optional_still_records_a_check_when_the_report_was_present_but_broken() {
        // The third disjunct of `acceptance.ts:1256`: `parsed.error !== ACCEPTANCE_REPORT_NOT_
        // FOUND`. A MALFORMED report is a real finding even for a caller that would have
        // accepted no report at all.
        let dir = temp_dir();
        let acceptance = attested_policy_requiring_no_report();
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "done\n```acceptance-report\n{ not json at all\n```",
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: true,
        })
        .await;
        assert_ne!(
            ledger.child_report_parse_error.as_deref(),
            Some(ACCEPTANCE_REPORT_NOT_FOUND),
            "premise: this must be a PARSE failure, not an absence"
        );
        assert_eq!(ledger.runtime_checks.len(), 1);
        assert_eq!(ledger.runtime_checks[0].id, "attestation");
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_mode_rejects_missing_required_evidence() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            // SUBA-076: `null` REMOVES the key, so `tests-added` is genuinely ABSENT. This test
            // previously passed `[]` for the same purpose, which no longer means the same thing:
            // an empty list is now `not-applicable` (pi `acceptance.ts:939` @v0.57.0) and does not
            // reject. "Missing" is what this test is named for, so it now says missing.
            output: &report_text(json!({"testsAddedOrUpdated": null}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        assert!(acceptance_failure_message(&ledger).unwrap().contains("tests-added evidence missing"));
    }

    /// SUBA-076, the other side of the same coin at LEDGER level: a child that honestly reports an
    /// EMPTY `testsAddedOrUpdated` is saying the question does not apply, not withholding evidence.
    /// pi scores that `not-applicable` (`acceptance.ts:939` @v0.57.0) and `evaluateAcceptance`
    /// rejects only on `failed`, so the run is ACCEPTED. This port rejected it, which is the bug.
    #[tokio::test]
    async fn checked_mode_accepts_an_honestly_empty_evidence_list() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({"testsAddedOrUpdated": []}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(
            ledger.status,
            AcceptanceLedgerStatus::Checked,
            "an empty list is not withheld evidence: {ledger:?}"
        );
        assert!(
            ledger.runtime_checks.iter().any(|check| {
                check.id == "evidence:tests-added"
                    && check.status == RuntimeCheckStatus::NotApplicable
                    && check.message == "tests-added evidence explicitly reported as not applicable."
            }),
            "the check must be RECORDED as not-applicable, not quietly dropped: {ledger:?}"
        );
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_mode_rejects_not_satisfied_required_criteria() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                criteria: Some(vec![CriterionInput::Gate(AcceptanceGate {
                    id: Some("regression".into()),
                    must: Some("Regression is covered".into()),
                    evidence: None,
                    severity: None,
                })]),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(
                json!({"criteriaSatisfied": [{"id": "regression", "status": "not-satisfied", "evidence": "test missing"}]}),
                "acceptance-report",
            ),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        assert!(acceptance_failure_message(&ledger)
            .unwrap()
            .contains("Required criterion 'regression' was reported as not-satisfied"));
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    /// G78 — a dynamic-fanout run whose inference recommends review. v0.34.0 escalated the
    /// LEVEL to `reviewed` and then downgraded the gate to `required: false` to avoid turning
    /// an explicit `checked` into a hard blocker; v0.43.0 has neither step, because the level
    /// stops at `checked` and the REQUIRED gate parks the run at `review-required` instead of
    /// rejecting it. `evidence_status` still records the `checked` that was earned.
    async fn dynamic_fanout_review_gate_parks_at_review_required_not_rejected() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement each dynamic item".into()),
            dynamic: true,
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(acceptance.level, AcceptanceLevel::Checked);
        assert!(matches!(
            &acceptance.review,
            Some(ReviewSetting::Gate(g)) if g.required == Some(true)
        ));
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(
                json!({"criteriaSatisfied": [
                    {"id": "criterion-1", "status": "satisfied", "evidence": "implemented"},
                    {"id": "criterion-2", "status": "satisfied", "evidence": "evidence returned"}
                ]}),
                "acceptance-report",
            ),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::ReviewRequired);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert!(acceptance_failure_message(&ledger).is_none());
        assert_eq!(
            ledger.review_result.as_ref().map(|r| r.status),
            Some(ReviewResultStatus::ReviewRequired)
        );
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    /// G78 — no reviewer result can ever be invented from the child's own evidence. With the
    /// gate explicitly switched OFF (`review: false`, which is falsy at `acceptance.ts:1318`)
    /// the review block does not run at all, so the ledger settles at its evidence level with
    /// NO `reviewResult` — it is never silently promoted to `reviewed`.
    async fn does_not_mark_reviewed_without_an_independent_reviewer_result() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                review: Some(ReviewSetting::Disabled(false)),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(acceptance.level, AcceptanceLevel::Checked);
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Checked);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert_eq!(ledger.review_result, None);
        assert!(acceptance_failure_message(&ledger).is_none());
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zero_child_aggregate_reports_do_not_fabricate_required_evidence() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement dynamic fanout fixes".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "",
            cwd: dir.path(),
            report: Some(aggregate_acceptance_report(&[], None)),
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        let msg = acceptance_failure_message(&ledger).unwrap();
        assert!(
            ["criterion", "changed-files", "tests-added", "commands-run", "validation-output", "no-staged-files"]
                .iter()
                .any(|needle| msg.contains(needle)),
            "unexpected message: {msg}"
        );
    }


    /// G78 — `acceptance.ts:1325`: `else if (acceptance.review.required !== false)`. The gate
    /// parks the ledger at `review-required` unless `required` is EXPLICITLY `false`, so a gate
    /// authored as a bare `{ agent: "reviewer" }` — no `required` key at all — is still a
    /// blocking one. Reading that condition as `required === true` would silently let every
    /// under-specified review gate through as `checked`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_review_gate_authored_without_required_still_parks_at_review_required() {
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    agent: Some("reviewer".into()),
                    focus: None,
                    // `required` DELIBERATELY absent — this is the whole point.
                    required: None,
                })),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(
            matches!(&acceptance.review, Some(ReviewSetting::Gate(g)) if g.required.is_none()),
            "premise: the resolved gate carries no `required` at all: {:?}",
            acceptance.review
        );

        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceLedgerStatus::ReviewRequired,
            "`required` defaults to REQUIRED (`!== false`), not to optional: {ledger:?}"
        );
        // Only `status` moves; the evidence the child actually earned is untouched.
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
        assert_eq!(
            ledger.review_result.as_ref().map(|r| r.status),
            Some(ReviewResultStatus::ReviewRequired)
        );
        assert_eq!(
            ledger
                .review_result
                .as_ref()
                .and_then(|r| r.findings.first())
                .map(|f| f.issue.as_str()),
            Some("Independent review has not been supplied."),
            "and the synthesized placeholder finding is upstream's (`acceptance.ts:1328-1332`)"
        );
        // `review-required` is not a rejection.
        assert!(acceptance_failure_message(&ledger).is_none());
    }


    /// G78 — `resolveEffectiveAcceptance`'s review resolution, `acceptance.ts:389`:
    /// `explicit.review !== undefined ? explicit.review : inferred.review`. An authored
    /// `review: false` must WIN over an inferred gate.
    ///
    /// The inferred gate here is real: a `dynamic` fanout context takes `inferLevel`'s risky
    /// branch, which returns `review: { agent: "reviewer", required: true }`
    /// (`acceptance.ts:114-120`) — the exact shape that parks a run at `review-required`
    /// forever when nothing supplies a reviewer. Falling back to `inferred.review`
    /// unconditionally would make `review: false` unauthorable on precisely the runs that need
    /// it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_explicit_review_false_overrides_an_inferred_required_gate() {
        let dir = temp_dir();
        // The same input WITHOUT the override, to prove the inferred gate is genuinely there.
        let inferred_only = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement each dynamic item".into()),
            dynamic: true,
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(
            matches!(
                &inferred_only.review,
                Some(ReviewSetting::Gate(g)) if g.required == Some(true)
            ),
            "premise: inference produces a REQUIRED review gate here: {:?}",
            inferred_only.review
        );

        let overridden = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement each dynamic item".into()),
            dynamic: true,
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                review: Some(ReviewSetting::Disabled(false)),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(
            overridden.review,
            Some(ReviewSetting::Disabled(false)),
            "an explicit `review: false` replaces the inferred gate outright"
        );
        assert_eq!(
            overridden.level, inferred_only.level,
            "and it changes NOTHING else — the level is still the inferred/explicit max"
        );

        // The behavioural consequence at the gate: `review: false` is falsy at
        // `acceptance.ts:1318`, so the review block never runs.
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &overridden,
            output: &report_text(
                json!({"criteriaSatisfied": [
                    {"id": "criterion-1", "status": "satisfied", "evidence": "implemented"},
                    {"id": "criterion-2", "status": "satisfied", "evidence": "evidence returned"}
                ]}),
                "acceptance-report",
            ),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(
            ledger.status,
            AcceptanceLedgerStatus::Checked,
            "with the gate switched off the run settles at its evidence level: {ledger:?}"
        );
        assert_eq!(ledger.review_result, None);
    }


    /// G79 — `validateStringArrayField`'s v0.43.0 tightening (`acceptance.ts:827` added
    /// `|| !item.trim()`): a BLANK entry is no longer admissible evidence.
    ///
    /// This asserts the RULE, not the message: a report whose `changedFiles` is `["   "]` must
    /// not parse at all, so a `checked` policy that requires `changed-files` evidence REJECTS
    /// the run instead of accepting a whitespace string as proof a file changed. The pre-v0.43.0
    /// check (`typeof item !== "string"`) admitted it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_blank_evidence_entry_is_not_admissible_evidence() {
        // The parser refuses the report outright.
        let blank = parse_acceptance_report(&report_text(
            json!({"changedFiles": ["   ", "\t\n"]}),
            "acceptance-report",
        ));
        assert!(
            blank.report.is_none(),
            "a blank array entry must invalidate the whole report: {blank:?}"
        );
        // Both entries are flagged, not just the first — the check is per-item.
        let err = blank.error.as_deref().unwrap();
        assert!(err.contains("changedFiles[0]"), "{err}");
        assert!(err.contains("changedFiles[1]"), "{err}");

        // ... and the same array with ONE real entry parses, so the rule is "blank", not
        // "array".
        let real = parse_acceptance_report(&report_text(
            json!({"changedFiles": ["src/file.rs"]}),
            "acceptance-report",
        ));
        assert_eq!(real.error, None, "{real:?}");

        // The consequence at the live gate: a policy gated on `changed-files` evidence rejects
        // the blank report rather than counting it.
        let dir = temp_dir();
        let acceptance = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                evidence: Some(vec![AcceptanceEvidenceKind::ChangedFiles]),
                ..Default::default()
            }),
            ..Default::default()
        });
        let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: &report_text(json!({"changedFiles": ["   "]}), "acceptance-report"),
            cwd: dir.path(),
            report: None,
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(
            ledger.status,
            AcceptanceLedgerStatus::Rejected,
            "a whitespace-only changedFiles entry is not evidence: {ledger:?}"
        );
        assert!(
            ledger
                .child_report_parse_error
                .as_deref()
                .unwrap()
                .contains("changedFiles[0]: expected non-empty string"),
            "{ledger:?}"
        );
    }

}
