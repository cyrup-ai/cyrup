//! Report-driven runtime checks (pi `acceptance.ts:911-966`), including the REAL
//! `git status --short` no-staged-files check.

use std::path::Path;

use super::report::normalize::normalized_token;
use super::types::{AcceptanceEvidenceKind, AcceptanceReport, AcceptanceRuntimeCheck, CriterionReport, CriterionStatus, GateSeverity, ResolvedAcceptanceGate, RuntimeCheckStatus};

// --------------------------------------------------------------------------------------------
// Report-driven runtime checks (acceptance.ts:911-966)
// --------------------------------------------------------------------------------------------

/// `reportEvidencePresent` (acceptance.ts:632-644).
fn report_evidence_present(report: &AcceptanceReport, kind: AcceptanceEvidenceKind) -> bool {
    match kind {
        AcceptanceEvidenceKind::ChangedFiles => {
            report.changed_files.as_ref().is_some_and(|v| !v.is_empty())
        }
        AcceptanceEvidenceKind::TestsAdded => report
            .tests_added_or_updated
            .as_ref()
            .is_some_and(|v| !v.is_empty()),
        AcceptanceEvidenceKind::CommandsRun => {
            report.commands_run.as_ref().is_some_and(|v| !v.is_empty())
        }
        AcceptanceEvidenceKind::ValidationOutput => report
            .validation_output
            .as_ref()
            .is_some_and(|v| !v.is_empty()),
        AcceptanceEvidenceKind::ResidualRisks => report.residual_risks.is_some(),
        AcceptanceEvidenceKind::NoStagedFiles => report.no_staged_files == Some(true),
        AcceptanceEvidenceKind::DiffSummary => report
            .diff_summary
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty()),
        AcceptanceEvidenceKind::ReviewFindings => report.review_findings.is_some(),
        AcceptanceEvidenceKind::ManualNotes => report
            .manual_notes
            .as_deref()
            .or(report.notes.as_deref())
            .is_some_and(|s| !s.trim().is_empty()),
    }
}

/// `checkCriteriaSatisfied` (acceptance.ts:911-919).
///
/// Public because the live gate ([`crate::exec::acceptance::evaluate_acceptance`]'s `Checked` rung) runs this
/// same check over [`crate::exec::acceptance::AcceptanceContract::criteria`] — there is exactly one criteria
/// rule and both evaluators must share it.
#[must_use]
pub fn check_criteria_satisfied(
    criteria: &[ResolvedAcceptanceGate],
    report: &AcceptanceReport,
) -> Vec<AcceptanceRuntimeCheck> {
    // `acceptance.ts:912-914` @v0.43.0: BOTH sides go through `normalizedToken`, so a declared
    // `c 1` matches a reported `C_1`. The report side is already normalized by
    // `normalize_criterion_report`; the DECLARED side is not, so it is normalized here.
    let reported: std::collections::HashMap<String, &CriterionReport> = report
        .criteria_satisfied
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            item.id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| (normalized_token(id), item))
        })
        .collect();
    criteria
        .iter()
        .filter(|criterion| criterion.severity != GateSeverity::Recommended)
        .map(|criterion| {
            let id = format!("criterion:{}", criterion.id);
            match reported.get(&normalized_token(&criterion.id)) {
                Option::None => AcceptanceRuntimeCheck {
                    id,
                    status: RuntimeCheckStatus::Failed,
                    message: format!(
                        "Required criterion '{}' was not reported.",
                        criterion.id
                    ),
                },
                Some(item) if item.status != CriterionStatus::Satisfied => AcceptanceRuntimeCheck {
                    id,
                    status: RuntimeCheckStatus::Failed,
                    message: format!(
                        "Required criterion '{}' was reported as {}.",
                        criterion.id,
                        criterion_status_str(item.status)
                    ),
                },
                Some(_) => AcceptanceRuntimeCheck {
                    id,
                    status: RuntimeCheckStatus::Passed,
                    message: format!("Required criterion '{}' satisfied.", criterion.id),
                },
            }
        })
        .collect()
}

fn criterion_status_str(status: CriterionStatus) -> &'static str {
    match status {
        CriterionStatus::Satisfied => "satisfied",
        CriterionStatus::NotSatisfied => "not-satisfied",
        CriterionStatus::NotApplicable => "not-applicable",
    }
}

/// `checkNoStagedFiles` (acceptance.ts:939-948) — REAL `git status --short` subprocess in `cwd`.
async fn check_no_staged_files(cwd: &Path) -> AcceptanceRuntimeCheck {
    let output = tokio::process::Command::new("git")
        .arg("status")
        .arg("--short")
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;
    let Ok(output) = output else {
        return AcceptanceRuntimeCheck {
            id: "no-staged-files".to_string(),
            status: RuntimeCheckStatus::NotApplicable,
            message: "git status unavailable; no staged-files check skipped".to_string(),
        };
    };
    if !output.status.success() {
        return AcceptanceRuntimeCheck {
            id: "no-staged-files".to_string(),
            status: RuntimeCheckStatus::NotApplicable,
            message: "git status unavailable; no staged-files check skipped".to_string(),
        };
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // pi: lines with length>=2 and line[0] !== " " and line[0] !== "?" are staged.
    let staged: Vec<&str> = stdout
        .split(['\n', '\r'])
        .filter(|line| {
            let mut chars = line.chars();
            let first = chars.next();
            line.chars().count() >= 2
                && !matches!(first, Some(' ') | Some('?'))
        })
        .collect();
    if staged.is_empty() {
        AcceptanceRuntimeCheck {
            id: "no-staged-files".to_string(),
            status: RuntimeCheckStatus::Passed,
            message: "No staged files detected.".to_string(),
        }
    } else {
        AcceptanceRuntimeCheck {
            id: "no-staged-files".to_string(),
            status: RuntimeCheckStatus::Failed,
            message: format!("Staged files present: {}", staged.join(", ")),
        }
    }
}

/// `runStructuralChecks` (acceptance.ts:950-966).
///
/// Takes the evidence list rather than the whole [`crate::exec::acceptance::model::types::ResolvedAcceptanceConfig`] — upstream reads
/// nothing else off `acceptance` (`for (const kind of acceptance.evidence)` plus the
/// `acceptance.evidence.includes("no-staged-files")` branch) — so the live gate, whose contract
/// is an [`crate::exec::acceptance::AcceptanceContract`] rather than a `ResolvedAcceptanceConfig`, can call this
/// exact function instead of growing a second copy of the evidence rules.
#[must_use]
pub async fn run_structural_checks(
    evidence: &[AcceptanceEvidenceKind],
    report: &AcceptanceReport,
    cwd: &Path,
) -> Vec<AcceptanceRuntimeCheck> {
    let mut checks: Vec<AcceptanceRuntimeCheck> = Vec::new();
    for kind in evidence {
        let present = report_evidence_present(report, *kind);
        checks.push(AcceptanceRuntimeCheck {
            id: format!("evidence:{}", kind.as_str()),
            status: if present {
                RuntimeCheckStatus::Passed
            } else {
                RuntimeCheckStatus::Failed
            },
            message: if present {
                format!("{} evidence present.", kind.as_str())
            } else {
                format!("{} evidence missing from child report.", kind.as_str())
            },
        });
    }
    if evidence.contains(&AcceptanceEvidenceKind::NoStagedFiles) {
        checks.push(check_no_staged_files(cwd).await);
    }
    checks
}
