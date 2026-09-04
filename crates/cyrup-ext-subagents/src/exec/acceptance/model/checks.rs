//! Report-driven runtime checks (pi `acceptance.ts:922-978` @v0.57.0 — `checkCriteriaSatisfied`,
//! `reportEvidenceStatus`, `checkNoStagedFiles`, `runStructuralChecks`), including the REAL
//! `git status --short` no-staged-files check.

use std::path::Path;

use super::report::normalize::normalized_token;
use super::types::{
    AcceptanceEvidenceKind, AcceptanceReport, AcceptanceRuntimeCheck, CriterionReport,
    CriterionStatus, GateSeverity, ResolvedAcceptanceGate, RuntimeCheckStatus,
};

// --------------------------------------------------------------------------------------------
// Report-driven runtime checks (acceptance.ts:922-978 @v0.57.0)
// --------------------------------------------------------------------------------------------

/// The shared `changed-files` / `tests-added` rule (pi `acceptance.ts:934-939` @v0.57.0).
///
/// An HONEST empty list is not withheld evidence — it is evidence that the question does not apply.
/// A reviewer persona, an oracle, and a genuine no-op task all legitimately change no files, and
/// scoring that `Failed` rejects work upstream accepts.
const fn tri_state_list_evidence(field: Option<&[String]>) -> RuntimeCheckStatus {
    match field {
        // pi's `!isStringArray(...)` arm. An ABSENT key is the only way to reach it here: the
        // report normalizer repairs a bare string into a one-element array
        // (`report/normalize.rs`'s `Value::String(_) => Value::Array(vec![field.clone()])`), and
        // any other non-array shape fails deserialization into the malformed-report path, which
        // never reaches this function at all.
        Option::None => RuntimeCheckStatus::Failed,
        Some([]) => RuntimeCheckStatus::NotApplicable,
        Some(_) => RuntimeCheckStatus::Passed,
    }
}

/// The binary rule the other seven evidence kinds share.
const fn passed_or_failed(present: bool) -> RuntimeCheckStatus {
    if present {
        RuntimeCheckStatus::Passed
    } else {
        RuntimeCheckStatus::Failed
    }
}

/// `reportEvidenceStatus` (pi `acceptance.ts:932` @v0.57.0 — the function was
/// `reportEvidencePresent`, returning a bool, when this port first followed it at v0.43.0).
///
/// Tri-state for `changed-files`/`tests-added`, binary for the other seven: upstream's own split,
/// not a simplification of it.
fn report_evidence_status(
    report: &AcceptanceReport,
    kind: AcceptanceEvidenceKind,
) -> RuntimeCheckStatus {
    match kind {
        AcceptanceEvidenceKind::ChangedFiles => {
            tri_state_list_evidence(report.changed_files.as_deref())
        }
        AcceptanceEvidenceKind::TestsAdded => {
            tri_state_list_evidence(report.tests_added_or_updated.as_deref())
        }
        AcceptanceEvidenceKind::CommandsRun => {
            passed_or_failed(report.commands_run.as_ref().is_some_and(|v| !v.is_empty()))
        }
        AcceptanceEvidenceKind::ValidationOutput => passed_or_failed(
            report
                .validation_output
                .as_ref()
                .is_some_and(|v| !v.is_empty()),
        ),
        // pi `isStringArray(report.residualRisks)` with NO length test — an empty list passes.
        AcceptanceEvidenceKind::ResidualRisks => passed_or_failed(report.residual_risks.is_some()),
        AcceptanceEvidenceKind::NoStagedFiles => {
            passed_or_failed(report.no_staged_files == Some(true))
        }
        AcceptanceEvidenceKind::DiffSummary => passed_or_failed(
            report
                .diff_summary
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
        ),
        // Likewise `isStringArray` only.
        AcceptanceEvidenceKind::ReviewFindings => {
            passed_or_failed(report.review_findings.is_some())
        }
        AcceptanceEvidenceKind::ManualNotes => passed_or_failed(
            report
                .manual_notes
                .as_deref()
                .or(report.notes.as_deref())
                .is_some_and(|s| !s.trim().is_empty()),
        ),
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
                    message: format!("Required criterion '{}' was not reported.", criterion.id),
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
            line.chars().count() >= 2 && !matches!(first, Some(' ') | Some('?'))
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

/// `runStructuralChecks` (pi `acceptance.ts:961` @v0.57.0).
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
        // pi `acceptance.ts:964` @v0.57.0: the REPORT-derived no-staged-files check is SKIPPED when
        // the child said nothing about it, leaving the parent's own `git status --short` — pushed
        // below whenever the kind is requested — as the sole authority. Upstream added this in
        // `bd5664a0 fix: trust parent staged-file acceptance check (#1385)`; it is absent at
        // v0.47.1. Without it a child that simply OMITS `noStagedFiles` is failed by the report
        // check even though the real check sitting in the very same list passed.
        if *kind == AcceptanceEvidenceKind::NoStagedFiles && report.no_staged_files.is_none() {
            continue;
        }
        let status = report_evidence_status(report, *kind);
        checks.push(AcceptanceRuntimeCheck {
            id: format!("evidence:{}", kind.as_str()),
            status,
            message: match status {
                RuntimeCheckStatus::Passed => format!("{} evidence present.", kind.as_str()),
                RuntimeCheckStatus::NotApplicable => format!(
                    "{} evidence explicitly reported as not applicable.",
                    kind.as_str()
                ),
                RuntimeCheckStatus::Failed => {
                    format!("{} evidence missing from child report.", kind.as_str())
                }
            },
        });
    }
    if evidence.contains(&AcceptanceEvidenceKind::NoStagedFiles) {
        checks.push(check_no_staged_files(cwd).await);
    }
    checks
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    /// Writes one of the two tri-state list fields onto a report.
    type SetListField = fn(&mut AcceptanceReport, Option<Vec<String>>);

    fn find<'a>(
        checks: &'a [AcceptanceRuntimeCheck],
        id: &str,
    ) -> Option<&'a AcceptanceRuntimeCheck> {
        checks.iter().find(|check| check.id == id)
    }

    /// SUBA-076 / pi `reportEvidenceStatus` (`acceptance.ts:934-939` @v0.57.0): `changed-files` and
    /// `tests-added` are TRI-state, and the middle arm is the whole point. A child that honestly
    /// reports `changedFiles: []` — a reviewer persona, an oracle, a genuine no-op task — is saying
    /// the question does not apply, not withholding evidence. Scored `Failed` (as this port did),
    /// that child's work is REJECTED even though upstream accepts it.
    #[tokio::test]
    async fn changed_files_and_tests_added_evidence_are_tri_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Function pointers rather than a match-on-kind, so adding a third tri-state kind cannot
        // silently fall through to a wrong field.
        let kinds: [(AcceptanceEvidenceKind, SetListField); 2] = [
            (AcceptanceEvidenceKind::ChangedFiles, |report, value| {
                report.changed_files = value;
            }),
            (AcceptanceEvidenceKind::TestsAdded, |report, value| {
                report.tests_added_or_updated = value;
            }),
        ];

        for (kind, set_field) in kinds {
            let cases = [
                (
                    Option::None,
                    RuntimeCheckStatus::Failed,
                    format!("{} evidence missing from child report.", kind.as_str()),
                ),
                (
                    Some(Vec::new()),
                    RuntimeCheckStatus::NotApplicable,
                    format!(
                        "{} evidence explicitly reported as not applicable.",
                        kind.as_str()
                    ),
                ),
                (
                    Some(vec!["src/a.rs".to_string()]),
                    RuntimeCheckStatus::Passed,
                    format!("{} evidence present.", kind.as_str()),
                ),
            ];

            for (value, expected_status, expected_message) in cases {
                let mut report = AcceptanceReport::default();
                set_field(&mut report, value.clone());
                let checks = run_structural_checks(&[kind], &report, dir.path()).await;

                let check = find(&checks, &format!("evidence:{}", kind.as_str()))
                    .expect("the requested evidence kind must produce a check");
                assert_eq!(
                    check.status,
                    expected_status,
                    "{} with {value:?} must score {expected_status:?}; got {check:?}",
                    kind.as_str()
                );
                assert_eq!(check.message, expected_message);
            }
        }
    }

    /// The consequence that makes the middle arm worth having: `not-applicable` is not a failure,
    /// so nothing in the list rejects. `evaluate_acceptance` rejects on
    /// `.any(status == Failed)`, which is what turns this into an accepted run.
    #[tokio::test]
    async fn an_honestly_empty_changed_files_list_produces_nothing_to_reject_on() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = AcceptanceReport {
            changed_files: Some(Vec::new()),
            ..AcceptanceReport::default()
        };
        let checks =
            run_structural_checks(&[AcceptanceEvidenceKind::ChangedFiles], &report, dir.path())
                .await;
        assert!(
            !checks
                .iter()
                .any(|check| check.status == RuntimeCheckStatus::Failed),
            "an honest empty list must leave nothing for the ledger to reject on: {checks:?}"
        );
    }

    /// SUBA-076 / pi `acceptance.ts:964` @v0.57.0: with `noStagedFiles` OMITTED the report-derived
    /// check is skipped entirely and the parent's own `git status --short` is the sole authority.
    /// Without the skip a child that simply says nothing is failed by the report check while the
    /// REAL check sitting in the very same list passes — the port contradicting itself.
    #[tokio::test]
    async fn an_omitted_no_staged_files_report_defers_to_the_real_git_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        let checks = run_structural_checks(
            &[AcceptanceEvidenceKind::NoStagedFiles],
            &AcceptanceReport::default(),
            dir.path(),
        )
        .await;

        assert!(
            find(&checks, "evidence:no-staged-files").is_none(),
            "the report-derived check must be skipped, not scored: {checks:?}"
        );
        assert_eq!(
            checks
                .iter()
                .filter(|check| check.id == "no-staged-files")
                .count(),
            1,
            "the REAL git check is still pushed, exactly once: {checks:?}"
        );
        // Asserted over the evidence checks only: whether the parent's `git status` passes is that
        // check's own business and depends on the worktree, but the skip must leave no evidence
        // failure behind for the ledger to reject on.
        assert!(
            !checks.iter().any(|check| check.id.starts_with("evidence:")
                && check.status == RuntimeCheckStatus::Failed),
            "{checks:?}"
        );
    }

    /// The skip is keyed on ABSENCE, not on the kind. A child that explicitly reports
    /// `noStagedFiles: false` is making a claim, and upstream scores it.
    #[tokio::test]
    async fn an_explicit_no_staged_files_false_is_still_scored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = AcceptanceReport {
            no_staged_files: Some(false),
            ..AcceptanceReport::default()
        };
        let checks = run_structural_checks(
            &[AcceptanceEvidenceKind::NoStagedFiles],
            &report,
            dir.path(),
        )
        .await;

        assert_eq!(
            find(&checks, "evidence:no-staged-files")
                .expect("not skipped")
                .status,
            RuntimeCheckStatus::Failed,
            "{checks:?}"
        );
        assert_eq!(
            checks
                .iter()
                .filter(|check| check.id == "no-staged-files")
                .count(),
            1,
            "the real check is pushed once either way: {checks:?}"
        );
    }

    /// The seven binary kinds keep their own rules. Two of them — `residual-risks` and
    /// `review-findings` — are pi `isStringArray(...)` with NO length test, so an EMPTY list
    /// PASSES; the two that do test length (`commands-run`, `validation-output`) still fail on one.
    /// Folding every list-shaped kind into the tri-state rule is the obvious way to get this wrong.
    #[tokio::test]
    async fn the_binary_evidence_kinds_keep_their_own_rules() {
        let dir = tempfile::tempdir().expect("tempdir");

        let empty_lists = AcceptanceReport {
            residual_risks: Some(Vec::new()),
            review_findings: Some(Vec::new()),
            validation_output: Some(Vec::new()),
            ..AcceptanceReport::default()
        };
        for kind in [
            AcceptanceEvidenceKind::ResidualRisks,
            AcceptanceEvidenceKind::ReviewFindings,
        ] {
            let checks = run_structural_checks(&[kind], &empty_lists, dir.path()).await;
            assert_eq!(
                checks[0].status,
                RuntimeCheckStatus::Passed,
                "{} is `isStringArray` only — an empty list is still evidence: {checks:?}",
                kind.as_str()
            );
        }

        let checks = run_structural_checks(
            &[AcceptanceEvidenceKind::ValidationOutput],
            &empty_lists,
            dir.path(),
        )
        .await;
        assert_eq!(
            checks[0].status,
            RuntimeCheckStatus::Failed,
            "validation-output DOES test length; an empty list is missing evidence: {checks:?}"
        );

        for kind in [
            AcceptanceEvidenceKind::ResidualRisks,
            AcceptanceEvidenceKind::ReviewFindings,
        ] {
            let checks =
                run_structural_checks(&[kind], &AcceptanceReport::default(), dir.path()).await;
            assert_eq!(
                checks[0].status,
                RuntimeCheckStatus::Failed,
                "an ABSENT {} is still missing evidence: {checks:?}",
                kind.as_str()
            );
        }
    }
}
