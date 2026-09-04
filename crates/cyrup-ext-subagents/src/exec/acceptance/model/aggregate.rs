//! `aggregateAcceptanceReport` (pi `acceptance.ts:671-711`): folding every child's report into
//! one parent report for a group run.

use super::types::{
    AcceptanceLedger, AcceptanceLedgerStatus, AcceptanceReport, CommandRunReport, CriterionReport,
    CriterionStatus,
};

// --------------------------------------------------------------------------------------------
// aggregateAcceptanceReport (acceptance.ts:671-711)
// --------------------------------------------------------------------------------------------

/// One child-run summary consumed by [`aggregate_acceptance_report`] (acceptance.ts:681-683 —
/// `Pick<SingleResult, "agent" | "acceptance" | "error" | "exitCode">`).
#[derive(Debug, Clone)]
pub struct AggregateChild {
    pub agent: String,
    pub acceptance: Option<AcceptanceLedger>,
    pub error: Option<String>,
    pub exit_code: i32,
}

pub(crate) fn trim_output(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Option::None;
    }
    if trimmed.chars().count() > 12_000 {
        let head: String = trimmed.chars().take(12_000).collect();
        Some(format!("{head}\n...[truncated]"))
    } else {
        Some(trimmed.to_string())
    }
}

fn unique_strings(items: Vec<Option<String>>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for text in items.into_iter().flatten() {
        let trimmed = text.trim();
        if !trimmed.is_empty() && !seen.iter().any(|s| s == trimmed) {
            seen.push(trimmed.to_string());
        }
    }
    seen
}

/// `aggregateAcceptanceReport` (acceptance.ts:1000-1030).
#[must_use]
pub fn aggregate_acceptance_report(
    results: &[AggregateChild],
    notes: Option<&str>,
) -> AcceptanceReport {
    let child_reports: Vec<&AcceptanceReport> = results
        .iter()
        .filter_map(|r| r.acceptance.as_ref().and_then(|l| l.child_report.as_ref()))
        .collect();
    let blockers: Vec<&AggregateChild> = results
        .iter()
        .filter(|r| {
            r.exit_code != 0
                || r.acceptance
                    .as_ref()
                    .is_some_and(|l| l.status == AcceptanceLedgerStatus::Rejected)
        })
        .collect();
    let successful = !results.is_empty() && blockers.is_empty();

    let mut criteria: Vec<CriterionReport> = vec![
        CriterionReport {
            id: Some("criterion-1".to_string()),
            status: if successful {
                CriterionStatus::Satisfied
            } else {
                CriterionStatus::NotSatisfied
            },
            evidence: if successful {
                format!(
                    "All {} dynamic child run(s) completed without child or acceptance blockers.",
                    results.len()
                )
            } else {
                "Dynamic fanout produced no accepted child evidence.".to_string()
            },
        },
        CriterionReport {
            id: Some("criterion-2".to_string()),
            status: if successful {
                CriterionStatus::Satisfied
            } else {
                CriterionStatus::NotSatisfied
            },
            evidence: if successful {
                "Collected child acceptance evidence for aggregate review.".to_string()
            } else {
                "Dynamic fanout produced no aggregate review evidence.".to_string()
            },
        },
    ];
    for (index, result) in results.iter().enumerate() {
        let child_ok = result.exit_code == 0
            && result
                .acceptance
                .as_ref()
                .is_none_or(|l| l.status != AcceptanceLedgerStatus::Rejected);
        let status_str = result
            .acceptance
            .as_ref()
            .map(|l| ledger_status_str(l.status))
            .unwrap_or("unreported");
        let err_suffix = result
            .error
            .as_deref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default();
        criteria.push(CriterionReport {
            id: Some(format!("child-{}", index + 1)),
            status: if child_ok {
                CriterionStatus::Satisfied
            } else {
                CriterionStatus::NotSatisfied
            },
            evidence: format!("{}: acceptance {status_str}{err_suffix}", result.agent),
        });
    }

    let changed_files = unique_strings(
        child_reports
            .iter()
            .flat_map(|r| r.changed_files.clone().unwrap_or_default())
            .map(Some)
            .collect(),
    );
    let tests = unique_strings(
        child_reports
            .iter()
            .flat_map(|r| r.tests_added_or_updated.clone().unwrap_or_default())
            .map(Some)
            .collect(),
    );
    let commands: Vec<CommandRunReport> = child_reports
        .iter()
        .flat_map(|r| r.commands_run.clone().unwrap_or_default())
        .collect();
    let validation = unique_strings(
        child_reports
            .iter()
            .flat_map(|r| r.validation_output.clone().unwrap_or_default())
            .map(Some)
            .collect(),
    );
    let mut residual_items: Vec<Option<String>> = child_reports
        .iter()
        .flat_map(|r| r.residual_risks.clone().unwrap_or_default())
        .map(Some)
        .collect();
    for blocker in &blockers {
        residual_items.push(Some(format!(
            "{}: {}",
            blocker.agent,
            blocker
                .error
                .clone()
                .unwrap_or_else(|| "child or acceptance gate failed".to_string())
        )));
    }
    let residual = unique_strings(residual_items);
    let review_findings = unique_strings(
        child_reports
            .iter()
            .flat_map(|r| r.review_findings.clone().unwrap_or_default())
            .map(Some)
            .collect(),
    );

    AcceptanceReport {
        criteria_satisfied: Some(criteria),
        changed_files: Some(changed_files),
        tests_added_or_updated: Some(tests),
        commands_run: Some(commands),
        validation_output: Some(validation),
        residual_risks: Some(residual),
        no_staged_files: Some(
            !child_reports.is_empty()
                && child_reports
                    .iter()
                    .all(|r| r.no_staged_files == Some(true)),
        ),
        diff_summary: Option::None,
        review_findings: Some(review_findings),
        manual_notes: Some(notes.map(str::to_string).unwrap_or_else(|| {
            format!(
                "Aggregated acceptance evidence from {} dynamic fanout child run(s).",
                results.len()
            )
        })),
        notes: notes.map(str::to_string),
    }
}

fn ledger_status_str(status: AcceptanceLedgerStatus) -> &'static str {
    match status {
        AcceptanceLedgerStatus::Pending => "pending",
        AcceptanceLedgerStatus::NotRequired => "not-required",
        AcceptanceLedgerStatus::Claimed => "claimed",
        AcceptanceLedgerStatus::Attested => "attested",
        AcceptanceLedgerStatus::Checked => "checked",
        AcceptanceLedgerStatus::Verified => "verified",
        AcceptanceLedgerStatus::ReviewRequired => "review-required",
        AcceptanceLedgerStatus::Reviewed => "reviewed",
        AcceptanceLedgerStatus::Accepted => "accepted",
        AcceptanceLedgerStatus::Rejected => "rejected",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::evaluate::EvaluateAcceptanceInput;
    use crate::exec::acceptance::model::evaluate::evaluate_acceptance;
    use crate::exec::acceptance::model::testsupport::attested_policy_requiring_no_report;
    use crate::exec::acceptance::model::testsupport::temp_dir;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_caller_supplied_report_is_revalidated_not_trusted() {
        // `validateAcceptanceReport(input.report)` (`acceptance.ts:1244-1248` @v0.43.0) — v0.34.0
        // took `input.report` verbatim. The dynamic-group gate is the caller that supplies one.
        let dir = temp_dir();
        let acceptance = attested_policy_requiring_no_report();
        let good = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "",
            cwd: dir.path(),
            report: Some(aggregate_acceptance_report(&[], Some("no children"))),
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(good.status, AcceptanceLedgerStatus::Attested);
        assert!(good.child_report.is_some());

        // A report carrying two criteria that normalize onto the SAME id fails
        // `validateAcceptanceReport` (`acceptance.ts:845,855-858`), so the supplied report is
        // REJECTED rather than recorded — which v0.34.0's verbatim-trust could not do.
        let mut invalid = aggregate_acceptance_report(&[], Some("no children"));
        invalid.criteria_satisfied = Some(vec![
            CriterionReport {
                id: Some("criterion-1".to_string()),
                status: CriterionStatus::Satisfied,
                evidence: "a".to_string(),
            },
            CriterionReport {
                id: Some("criterion-1".to_string()),
                status: CriterionStatus::Satisfied,
                evidence: "b".to_string(),
            },
        ]);
        let bad = evaluate_acceptance(EvaluateAcceptanceInput {
            acceptance: &acceptance,
            output: "",
            cwd: dir.path(),
            report: Some(invalid),
            file_output: None,
            review_result: None,
            memo: None,
            report_optional: false,
        })
        .await;
        assert_eq!(bad.status, AcceptanceLedgerStatus::Rejected);
        assert!(
            bad.child_report_parse_error
                .as_deref()
                .unwrap_or_default()
                .starts_with("Failed to parse acceptance-report: Invalid acceptance-report: "),
            "got {:?}",
            bad.child_report_parse_error
        );
    }
}
