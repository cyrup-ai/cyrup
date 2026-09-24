//! SUBA-105 — `acceptance.report`: whether a structured-output child hands its acceptance report
//! to the parent INSIDE its `structured_output` call (pi `resolveAcceptanceReportMode` /
//! `validateAcceptanceReportMode`, `runs/shared/acceptance.ts:189-197,423-430` @v0.68.0).
//!
//! The toggle only means anything for a step that declared an `outputSchema`: such a child
//! finishes by calling `structured_output`, and the report rides in that call's `acceptanceReport`
//! argument rather than in a fenced `acceptance-report` block in its prose. The three modes are:
//!
//! | authored | mode | the `structured_output` tool |
//! |---|---|---|
//! | `acceptance: false` or `report: "off"` | [`AcceptanceReportMode::Off`] | no `acceptanceReport` argument; the report (if any) is read from prose |
//! | no `report` key | [`AcceptanceReportMode::Optional`] | `acceptanceReport` accepted, not required |
//! | `report: "on"` | [`AcceptanceReportMode::Required`] | `acceptanceReport` required and validated before the call is accepted |

use serde_json::Value;

/// pi `ResolvedAcceptanceReportMode` (`acceptance.ts:189` @v0.68.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcceptanceReportMode {
    /// `"off"` — no structured acceptance report channel at all.
    Off,
    /// `"optional"` — the default: the channel exists, the child may leave it empty.
    #[default]
    Optional,
    /// `"required"` — `report: "on"`: a `structured_output` call without a valid
    /// `acceptanceReport` is refused.
    Required,
}

impl AcceptanceReportMode {
    /// The wire spelling (`"off"`/`"optional"`/`"required"`), also used for the child env hand-off.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    /// Inverse of [`Self::as_str`]. Anything else is `None`.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "optional" => Some(Self::Optional),
            "required" => Some(Self::Required),
            _ => None,
        }
    }
}

/// pi `resolveAcceptanceReportMode` (`acceptance.ts:192-197` @v0.68.0):
///
/// ```text
/// const report = value && typeof value === "object" && !Array.isArray(value) ? value.report : undefined;
/// return value === false || report === "off" ? "off" : report === "on" ? "required" : "optional";
/// ```
///
/// `Value::Null` is upstream's `undefined` (no policy at all), which is `optional`.
#[must_use]
pub fn resolve_acceptance_report_mode(input: &Value) -> AcceptanceReportMode {
    let report = input
        .as_object()
        .and_then(|map| map.get("report"))
        .and_then(Value::as_str);
    if *input == Value::Bool(false) || report == Some("off") {
        AcceptanceReportMode::Off
    } else if report == Some("on") {
        AcceptanceReportMode::Required
    } else {
        AcceptanceReportMode::Optional
    }
}

/// pi `"report" in acceptance` (`acceptance.ts:428`): whether the policy object DECLARED the key at
/// all — `report: "off"` included, which is why this is not `mode != Optional`.
#[must_use]
pub fn acceptance_declares_report(input: &Value) -> bool {
    input
        .as_object()
        .is_some_and(|map| map.contains_key("report"))
}

/// The refusal [`validate_acceptance_report_mode`] produces, for a `path_label` such as
/// `acceptance` or `tasks[0].acceptance` (`acceptance.ts:429` @v0.68.0).
#[must_use]
pub fn acceptance_report_requires_output_schema_message(path_label: &str) -> String {
    format!("{path_label}.report requires outputSchema.")
}

/// pi `validateAcceptanceReportMode` (`acceptance.ts:423-430` @v0.68.0): a policy that declares
/// `report` (either value) on a step with no `outputSchema` (absent or `false`) is refused — the
/// toggle only has a channel to act on when the child finishes through `structured_output`.
#[must_use]
pub fn validate_acceptance_report_mode(
    acceptance: &Value,
    output_schema: Option<&Value>,
    path_label: &str,
) -> Vec<String> {
    if !acceptance_declares_report(acceptance) {
        return Vec::new();
    }
    let has_schema =
        output_schema.is_some_and(|schema| !schema.is_null() && *schema != Value::Bool(false));
    if has_schema {
        Vec::new()
    } else {
        vec![acceptance_report_requires_output_schema_message(path_label)]
    }
}

/// pi `validateExecutionAcceptance`'s `report` half (`acceptance.ts:404-421` @v0.68.0): run
/// [`validate_acceptance_report_mode`] over every policy site a `subagent` dispatch declares, each
/// against the `outputSchema` beside it, with upstream's per-site path labels — the top-level
/// call, each `tasks[i]`, each `chain[i]`, and each `chain[i].parallel[j]` (array form) or
/// `chain[i].parallel` (dynamic-template object form).
#[must_use]
pub fn validate_execution_acceptance_report_modes(
    acceptance: Option<&Value>,
    output_schema: Option<&Value>,
    tasks: &[Value],
    chain: &[Value],
) -> Vec<String> {
    let site = |item: &Value, label: String| {
        validate_acceptance_report_mode(
            item.get("acceptance").unwrap_or(&Value::Null),
            item.get("outputSchema"),
            &label,
        )
    };
    let mut errors = validate_acceptance_report_mode(
        acceptance.unwrap_or(&Value::Null),
        output_schema,
        "acceptance",
    );
    for (index, task) in tasks.iter().enumerate() {
        errors.extend(site(task, format!("tasks[{index}].acceptance")));
    }
    for (step_index, step) in chain.iter().enumerate() {
        errors.extend(site(step, format!("chain[{step_index}].acceptance")));
        match step.get("parallel") {
            Some(Value::Array(tasks)) => {
                for (task_index, task) in tasks.iter().enumerate() {
                    errors.extend(site(
                        task,
                        format!("chain[{step_index}].parallel[{task_index}].acceptance"),
                    ));
                }
            }
            // `else if (step.parallel)` — a falsy `null`/`false` is skipped.
            Some(template) if !template.is_null() && *template != Value::Bool(false) => {
                errors.extend(site(
                    template,
                    format!("chain[{step_index}].parallel.acceptance"),
                ));
            }
            _ => {}
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use serde_json::json;

    /// `resolveAcceptanceReportMode`'s whole truth table (`acceptance.ts:192-197` @v0.68.0).
    #[test]
    fn report_mode_follows_upstreams_truth_table() {
        use AcceptanceReportMode::{Off, Optional, Required};
        assert_eq!(resolve_acceptance_report_mode(&Value::Null), Optional);
        assert_eq!(resolve_acceptance_report_mode(&json!(false)), Off);
        assert_eq!(resolve_acceptance_report_mode(&json!("checked")), Optional);
        assert_eq!(
            resolve_acceptance_report_mode(&json!({"level": "checked"})),
            Optional
        );
        assert_eq!(
            resolve_acceptance_report_mode(&json!({"report": "off"})),
            Off
        );
        assert_eq!(
            resolve_acceptance_report_mode(&json!({"report": "on"})),
            Required
        );
    }

    /// `validateAcceptanceReportMode` refuses the KEY, not the value: `report: "off"` with no
    /// schema is refused exactly as `report: "on"` is (`"report" in acceptance`,
    /// `acceptance.ts:428`), and `outputSchema: false` counts as no schema (`:429`).
    #[test]
    fn a_declared_report_without_an_output_schema_is_refused() {
        let schema = json!({"type": "object"});
        for report in ["on", "off"] {
            let policy = json!({"level": "checked", "report": report});
            assert_eq!(
                validate_acceptance_report_mode(&policy, None, "tasks[1].acceptance"),
                vec!["tasks[1].acceptance.report requires outputSchema.".to_string()]
            );
            assert_eq!(
                validate_acceptance_report_mode(&policy, Some(&json!(false)), "acceptance"),
                vec!["acceptance.report requires outputSchema.".to_string()]
            );
            assert!(
                validate_acceptance_report_mode(&policy, Some(&schema), "acceptance").is_empty()
            );
        }
        assert!(
            validate_acceptance_report_mode(&json!({"level": "checked"}), None, "acceptance")
                .is_empty()
        );
        assert!(validate_acceptance_report_mode(&json!(false), None, "acceptance").is_empty());
    }

    /// pi "requires outputSchema for explicit structured acceptance report mode"
    /// (`acceptance.test.ts:1264-1290` @v0.68.0), fixture and expectation verbatim: every site is
    /// checked against ITS OWN schema, `report: "off"` is refused like `"on"`, and each error
    /// carries its site's path.
    #[test]
    fn every_dispatch_site_is_checked_against_its_own_output_schema() {
        let schema = json!({"type": "object"});
        let on = json!({"level": "checked", "report": "on"});
        let off = json!({"level": "checked", "report": "off"});
        let tasks = vec![
            json!({"acceptance": off}),
            json!({"outputSchema": schema, "acceptance": on}),
            json!({"outputSchema": false, "acceptance": on}),
        ];
        let chain = vec![
            json!({"acceptance": on}),
            json!({"outputSchema": schema, "acceptance": off}),
            json!({"parallel": [{"acceptance": on}, {"outputSchema": schema, "acceptance": on}]}),
            json!({"parallel": {"acceptance": off}}),
        ];
        assert_eq!(
            validate_execution_acceptance_report_modes(Some(&on), None, &tasks, &chain),
            vec![
                "acceptance.report requires outputSchema.".to_string(),
                "tasks[0].acceptance.report requires outputSchema.".to_string(),
                "tasks[2].acceptance.report requires outputSchema.".to_string(),
                "chain[0].acceptance.report requires outputSchema.".to_string(),
                "chain[2].parallel[0].acceptance.report requires outputSchema.".to_string(),
                "chain[3].parallel.acceptance.report requires outputSchema.".to_string(),
            ]
        );
    }
}
