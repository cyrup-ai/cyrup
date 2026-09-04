//! `formatAcceptancePrompt` (pi `acceptance.ts:403-457`): the acceptance block appended to a
//! child's task text, and the report example it instructs the child to emit.

use super::types::{AcceptanceLevel, ResolvedAcceptanceConfig, ReviewSetting};

// --------------------------------------------------------------------------------------------
// formatAcceptancePrompt (acceptance.ts:403-457)
// --------------------------------------------------------------------------------------------

/// The `acceptance-report` example object, rendered exactly as pi's
/// `JSON.stringify(obj, null, 2)` produces it (acceptance.ts:333-344) — hand-written so the
/// bytes (key order + 2-space indent) match pi's child-facing prompt verbatim.
const ACCEPTANCE_REPORT_EXAMPLE: &str = r#"{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}"#;

/// `acceptanceRequiresChildReport` (`acceptance.ts:403-405` @v0.43.0): whether this policy
/// declares anything that can only be checked against the child's own structured report.
///
/// It is exactly `criteria.length > 0 || evidence.length > 0`. Both of upstream's readers use
/// it to avoid demanding a report the child was never told to write:
/// [`format_acceptance_prompt`] suppresses the whole contract block for a `report_optional`
/// caller whose policy requires none (`:409`), and [`crate::exec::acceptance::model::evaluate::evaluate_acceptance`] declines to reject a
/// `report_optional` run for a merely-absent report (`:1251,1256`).
#[must_use]
pub fn acceptance_requires_child_report(acceptance: &ResolvedAcceptanceConfig) -> bool {
    !acceptance.criteria.is_empty() || !acceptance.evidence.is_empty()
}

/// `formatAcceptancePrompt` (`acceptance.ts:407-448` @v0.43.0).
///
/// `report_optional` is upstream's `options.reportOptional` (`:407`): a caller that is not
/// insisting on a structured report gets NO contract block at all when the policy declares
/// neither criteria nor evidence (`:409`), which is the prompt half of the same flag
/// [`crate::exec::acceptance::model::evaluate::EvaluateAcceptanceInput::report_optional`] carries into the gate.
#[must_use]
pub fn format_acceptance_prompt(
    acceptance: &ResolvedAcceptanceConfig,
    report_optional: bool,
) -> String {
    if acceptance.level == AcceptanceLevel::None {
        return String::new();
    }
    if report_optional && !acceptance_requires_child_report(acceptance) {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        String::new(),
        crate::exec::acceptance::lattice::inject::ACCEPTANCE_CONTRACT_HEADING.to_string(),
        format!("Acceptance level: {}", acceptance.level.as_str()),
        "Completion is not accepted from prose alone. End with a structured acceptance report."
            .to_string(),
        String::new(),
        "Criteria:".to_string(),
    ];
    if acceptance.criteria.is_empty() {
        lines.push("- Return the requested result.".to_string());
    } else {
        for criterion in &acceptance.criteria {
            lines.push(format!("- {}: {}", criterion.id, criterion.must));
        }
    }
    lines.push(String::new());
    let evidence_list = if acceptance.evidence.is_empty() {
        "none".to_string()
    } else {
        acceptance
            .evidence
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    lines.push(format!("Required evidence: {evidence_list}"));

    if !acceptance.verify.is_empty() {
        lines.push(String::new());
        lines.push("Runtime verification commands configured by parent:".to_string());
        for command in &acceptance.verify {
            lines.push(format!("- {}: {}", command.id, command.command));
        }
    }
    if let Some(ReviewSetting::Gate(gate)) = &acceptance.review {
        lines.push(String::new());
        let requiredness = if gate.required == Some(false) {
            "optional"
        } else {
            "required"
        };
        let by = gate
            .agent
            .as_deref()
            .map(|a| format!(" by {a}"))
            .unwrap_or_default();
        lines.push(format!("Review gate: {requiredness}{by}."));
        if let Some(focus) = &gate.focus {
            lines.push(format!("Review focus: {focus}"));
        }
    }
    if !acceptance.stop_rules.is_empty() {
        lines.push(String::new());
        lines.push("Stop rules:".to_string());
        for rule in &acceptance.stop_rules {
            lines.push(format!("- {rule}"));
        }
    }
    lines.push(String::new());
    lines.push(
        "Finish with a fenced JSON block tagged `acceptance-report` in this shape:".to_string(),
    );
    lines.push(
        "Use empty arrays when no items apply; array fields contain strings unless object entries are shown."
            .to_string(),
    );
    lines.push("```acceptance-report".to_string());
    lines.push(ACCEPTANCE_REPORT_EXAMPLE.to_string());
    lines.push("```".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::evaluate::EvaluateAcceptanceInput;
    use crate::exec::acceptance::model::evaluate::evaluate_acceptance;
    use crate::exec::acceptance::model::level::AcceptanceResolveInput;
    use crate::exec::acceptance::model::testsupport::attested_policy_requiring_no_report;
    use crate::exec::acceptance::model::testsupport::cfg;
    use crate::exec::acceptance::model::testsupport::resolve;
    use crate::exec::acceptance::model::testsupport::temp_dir;
    use crate::exec::acceptance::model::types::AcceptanceConfig;
    use crate::exec::acceptance::model::types::AcceptanceEvidenceStatus;
    use crate::exec::acceptance::model::types::AcceptanceGate;
    use crate::exec::acceptance::model::types::AcceptanceLedgerStatus;
    use crate::exec::acceptance::model::types::CriterionInput;

    // ---- formatAcceptancePrompt ----

    #[test]
    fn formats_a_standardized_child_prompt_section() {
        let resolved = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                criteria: Some(vec![CriterionInput::Text("Patch the bug".into())]),
                stop_rules: Some(vec!["Do not stop after analysis".into()]),
                ..Default::default()
            }),
            ..Default::default()
        });
        let prompt = format_acceptance_prompt(&resolved, false);
        assert!(prompt.contains("## Acceptance Contract"));
        assert!(prompt.contains("Acceptance level: checked"));
        assert!(prompt.contains("Patch the bug"));
        assert!(prompt.contains("```acceptance-report"));
        assert!(prompt.contains("array fields contain strings"));
        assert!(prompt.contains("\"reviewFindings\": [\n    \"blocker:"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn report_optional_still_records_a_check_when_the_policy_needs_a_report() {
        // `needsReport` (`acceptance.ts:1251,1256`): a policy WITH criteria demands one however
        // optional the caller thinks the report is — the check is pushed, the early return is
        // skipped, and the combined check at `:1308-1312` is what rejects.
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
        assert!(acceptance_requires_child_report(&acceptance), "premise");
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
        assert_eq!(ledger.runtime_checks.len(), 1);
        assert_eq!(ledger.runtime_checks[0].id, "attestation");
        assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
    }

    #[test]
    fn report_optional_suppresses_the_contract_prompt_only_when_no_report_is_required() {
        // `formatAcceptancePrompt`'s own `reportOptional` guard (`acceptance.ts:409`).
        let none_required = attested_policy_requiring_no_report();
        assert_eq!(format_acceptance_prompt(&none_required, true), "");
        assert!(
            !format_acceptance_prompt(&none_required, false).is_empty(),
            "a non-report-optional caller still gets the whole contract block"
        );

        let with_criteria = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement a fix".into()),
            explicit: cfg(AcceptanceConfig {
                level: Some(AcceptanceLevel::Checked),
                criteria: Some(vec![CriterionInput::Text("Ship the fix".into())]),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(acceptance_requires_child_report(&with_criteria), "premise");
        assert!(
            !format_acceptance_prompt(&with_criteria, true).is_empty(),
            "a policy that declares criteria is always prompted, reportOptional or not"
        );
    }
}
