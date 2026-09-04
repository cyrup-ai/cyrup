//! `validateAcceptanceInput` (pi `acceptance.ts:138-249`): validating a caller-supplied
//! `acceptance` value before it is ever lowered or dispatched.

use serde_json::Value;

use super::report::normalize::normalized_token;
use super::types::AcceptanceEvidenceKind;

// --------------------------------------------------------------------------------------------
// validateAcceptanceInput (acceptance.ts:138-249)
// --------------------------------------------------------------------------------------------

const VALID_LEVELS: &[&str] = &["auto", "none", "attested", "checked", "verified"];
/// `EXPLICIT_REVIEWED_UNAVAILABLE` (`acceptance.ts:54` @v0.43.0) — verbatim, including the
/// leading space the two call sites supply by interpolation (`${pathLabel} ${…}` /
/// `${pathLabel}.level ${…}`).
pub const EXPLICIT_REVIEWED_UNAVAILABLE: &str = "is an achieved status, not a requestable acceptance level. For a read-only reviewer call, omit acceptance. To require independent review of a writer result, use acceptance.review.required and orchestrate the reviewer separately.";
/// `VALID_EVIDENCE_KINDS` (`acceptance.ts:36-46` @v0.43.0), in upstream's declaration order —
/// which is load-bearing, because [`acceptance_evidence_help`] joins it verbatim into the
/// guidance suffix v0.43.0 appends to every evidence-kind validation message.
const VALID_EVIDENCE_KINDS: &[AcceptanceEvidenceKind] = &[
    AcceptanceEvidenceKind::ChangedFiles,
    AcceptanceEvidenceKind::TestsAdded,
    AcceptanceEvidenceKind::CommandsRun,
    AcceptanceEvidenceKind::ValidationOutput,
    AcceptanceEvidenceKind::ResidualRisks,
    AcceptanceEvidenceKind::NoStagedFiles,
    AcceptanceEvidenceKind::DiffSummary,
    AcceptanceEvidenceKind::ReviewFindings,
    AcceptanceEvidenceKind::ManualNotes,
];

/// `ACCEPTANCE_OBJECT_EXAMPLE` (`acceptance.ts:49` @v0.43.0) — appended to the "must be a
/// string level, false, or an object." message so a caller that passed the wrong SHAPE is shown
/// a working one rather than only told it was wrong.
const ACCEPTANCE_OBJECT_EXAMPLE: &str =
    "Example: { level: \"checked\", evidence: [\"commands-run\", \"changed-files\"] }.";

/// `ACCEPTANCE_EVIDENCE_HELP` (`acceptance.ts:48` @v0.43.0):
/// `Supported evidence kinds: ${VALID_EVIDENCE_KINDS.join(", ")}. ${ACCEPTANCE_OBJECT_EXAMPLE}`.
///
/// Built from [`VALID_EVIDENCE_KINDS`] rather than written out, so adding an evidence kind
/// cannot leave the help text listing a stale set — upstream derives it the same way.
#[must_use]
fn acceptance_evidence_help() -> String {
    let kinds: Vec<&str> = VALID_EVIDENCE_KINDS.iter().map(|k| k.as_str()).collect();
    format!(
        "Supported evidence kinds: {}. {ACCEPTANCE_OBJECT_EXAMPLE}",
        kinds.join(", ")
    )
}

/// `unsupportedEvidenceKindMessage` (`acceptance.ts:171-174` @v0.43.0) — v0.43.0 replaced the
/// bare `<path> is not a supported evidence kind.` with this: it ECHOES the offending value
/// when it was a string (`const value = typeof item === "string" ? ` "${item}"` : ""`) and
/// appends [`acceptance_evidence_help`].
#[must_use]
fn unsupported_evidence_kind_message(path_label: &str, item: &Value) -> String {
    let echoed = match item.as_str() {
        Some(text) => format!(" \"{text}\""),
        Option::None => String::new(),
    };
    format!(
        "{path_label}{echoed} is not a supported evidence kind. {}",
        acceptance_evidence_help()
    )
}

const ACCEPTANCE_CONFIG_KEYS: &[&str] = &[
    "level",
    "criteria",
    "evidence",
    "verify",
    "review",
    "stopRules",
    "reason",
];
const ACCEPTANCE_GATE_KEYS: &[&str] = &["id", "must", "evidence", "severity"];
const ACCEPTANCE_VERIFY_KEYS: &[&str] =
    &["id", "command", "timeoutMs", "cwd", "env", "allowFailure"];
const ACCEPTANCE_REVIEW_KEYS: &[&str] = &["agent", "focus", "required"];

/// `validateAcceptanceInput` (acceptance.ts:138-249). `input` is the raw JSON value the caller
/// supplied; `Value::Null` stands for pi's `undefined` (returns no errors).
#[must_use]
pub fn validate_acceptance_input(input: &Value, path_label: &str) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    match input {
        Value::Null => return errors,
        Value::Bool(false) => return errors,
        // `acceptance.ts:180-185` @v0.43.0. Every bare level string except `auto`, `attested`
        // and `checked` is now an error: `reviewed` is not a level at all, `none` needs a
        // reason, and `verified` needs runtime commands — which is precisely
        // `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified"> | false |
        // AcceptanceConfig` (`shared/types.ts:684-685`) restated as messages.
        Value::String(s) => {
            if s == "reviewed" {
                errors.push(format!("{path_label} {EXPLICIT_REVIEWED_UNAVAILABLE}"));
            } else if !VALID_LEVELS.contains(&s.as_str()) {
                errors.push(format!("{path_label} has invalid level '{s}'."));
            } else if s == "none" {
                errors.push(format!(
                    "{path_label} level \"none\" requires a reason; use {{ level: \"none\", reason: \"...\" }}."
                ));
            } else if s == "verified" {
                errors.push(format!(
                    "{path_label} level \"verified\" requires object form with at least one runtime verify command. Use level \"checked\" or provide a non-empty acceptance.verify array."
                ));
            }
            return errors;
        }
        Value::Object(_) => {}
        _ => {
            // `acceptance.ts:188` @v0.43.0 — the message now carries
            // `ACCEPTANCE_OBJECT_EXAMPLE`.
            errors.push(format!(
                "{path_label} must be a string level, false, or an object. {ACCEPTANCE_OBJECT_EXAMPLE}"
            ));
            return errors;
        }
    }
    let Value::Object(map) = input else {
        return errors;
    };
    for key in map.keys() {
        if !ACCEPTANCE_CONFIG_KEYS.contains(&key.as_str()) {
            errors.push(format!("{path_label}.{key} is not supported."));
        }
    }
    // `acceptance.ts:195-199` @v0.43.0.
    if map.get("level").and_then(Value::as_str) == Some("reviewed") {
        errors.push(format!(
            "{path_label}.level {EXPLICIT_REVIEWED_UNAVAILABLE}"
        ));
    } else if let Some(level) = map.get("level")
        && !level.as_str().is_some_and(|l| VALID_LEVELS.contains(&l))
    {
        errors.push(format!(
            "{path_label}.level must be one of auto, none, attested, checked, verified."
        ));
    }
    if map.get("level").and_then(Value::as_str) == Some("none")
        && map
            .get("reason")
            .and_then(Value::as_str)
            .is_none_or(|r| r.trim().is_empty())
    {
        errors.push(format!(
            "{path_label}.reason is required when level is none."
        ));
    }
    if let Some(reason) = map.get("reason")
        && !reason.is_string()
    {
        errors.push(format!("{path_label}.reason must be a string."));
    }
    validate_criteria_input(&mut errors, map.get("criteria"), path_label);
    // `acceptance.ts:239-247` @v0.43.0 — both arms now carry the guidance suffix.
    match map.get("evidence") {
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                if item
                    .as_str()
                    .and_then(AcceptanceEvidenceKind::from_wire)
                    .is_none()
                {
                    errors.push(unsupported_evidence_kind_message(
                        &format!("{path_label}.evidence[{index}]"),
                        item,
                    ));
                }
            }
        }
        Some(_) => errors.push(format!(
            "{path_label}.evidence must be an array. {}",
            acceptance_evidence_help()
        )),
        Option::None => {}
    }
    // `acceptance.ts:248-252` @v0.43.0: an object-form `verified` policy MUST carry at least one
    // runtime command — the level's whole meaning is "the orchestrator ran something", so a
    // command-less `verified` could only ever have been satisfied by the child's own claim.
    let verified_without_commands = map.get("level").and_then(Value::as_str) == Some("verified")
        && !map
            .get("verify")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
    if verified_without_commands {
        errors.push(format!(
            "{path_label}.verify must contain at least one runtime command when level is verified. Use level \"checked\" or provide a non-empty acceptance.verify array."
        ));
    }
    // Upstream's `else if` suppresses only the generic "must be an array" message; the
    // per-command checks below it are an unconditional `if (Array.isArray(value.verify))`.
    validate_verify_input(
        &mut errors,
        map.get("verify"),
        path_label,
        !verified_without_commands,
    );
    validate_review_input(&mut errors, map.get("review"), path_label);
    match map.get("stopRules") {
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                if !item.is_string() {
                    errors.push(format!("{path_label}.stopRules[{index}] must be a string."));
                }
            }
        }
        Some(_) => errors.push(format!("{path_label}.stopRules must be an array.")),
        Option::None => {}
    }
    errors
}

/// `validateAcceptanceInput`'s `criteria` block (`acceptance.ts:204-238` @v0.43.0).
///
/// v0.43.0 added the `criterionIds` set (`:206,218-224`): two gates whose ids differ only in
/// case, whitespace or `_`-vs-`-` spelling collapse onto ONE
/// [`normalized_token`] — which is the key
/// [`crate::exec::acceptance::model::checks::check_criteria_satisfied`] matches the child's reported criteria against — so the second
/// gate would silently shadow the first and its `must` would never be enforced. The check is
/// deliberately inside the `else` of the `id is required` test: a blank/absent id has already
/// been reported and must not additionally claim to duplicate the empty token.
fn validate_criteria_input(errors: &mut Vec<String>, criteria: Option<&Value>, path_label: &str) {
    match criteria {
        Option::None => {}
        Some(Value::Array(items)) => {
            // `const criterionIds = new Set<string>()` (`acceptance.ts:206`) — scoped to ONE
            // `criteria[]` array, so two different policies may reuse an id freely.
            let mut criterion_ids: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (index, criterion) in items.iter().enumerate() {
                if criterion.is_string() {
                    continue;
                }
                let cpath = format!("{path_label}.criteria[{index}]");
                let Value::Object(gate) = criterion else {
                    errors.push(format!("{cpath} must be a string or an object."));
                    continue;
                };
                for key in gate.keys() {
                    if !ACCEPTANCE_GATE_KEYS.contains(&key.as_str()) {
                        errors.push(format!("{cpath}.{key} is not supported."));
                    }
                }
                // `acceptance.ts:218-224` @v0.43.0.
                match gate
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|i| !i.trim().is_empty())
                {
                    Option::None => errors.push(format!("{cpath}.id is required.")),
                    Some(id) => {
                        let normalized_id = normalized_token(id);
                        if criterion_ids.contains(&normalized_id) {
                            errors.push(format!(
                                "{cpath}.id duplicates normalized criterion id '{normalized_id}'."
                            ));
                        }
                        criterion_ids.insert(normalized_id);
                    }
                }
                if gate
                    .get("must")
                    .and_then(Value::as_str)
                    .is_none_or(|m| m.trim().is_empty())
                {
                    errors.push(format!("{cpath}.must is required."));
                }
                // `acceptance.ts:226-233` @v0.43.0 — both arms carry the guidance suffix.
                match gate.get("evidence") {
                    Some(Value::Array(evs)) => {
                        for (ei, item) in evs.iter().enumerate() {
                            if item
                                .as_str()
                                .and_then(AcceptanceEvidenceKind::from_wire)
                                .is_none()
                            {
                                errors.push(unsupported_evidence_kind_message(
                                    &format!("{cpath}.evidence[{ei}]"),
                                    item,
                                ));
                            }
                        }
                    }
                    Some(_) => errors.push(format!(
                        "{cpath}.evidence must be an array. {}",
                        acceptance_evidence_help()
                    )),
                    Option::None => {}
                }
                if let Some(sev) = gate.get("severity")
                    && !matches!(sev.as_str(), Some("required") | Some("recommended"))
                {
                    errors.push(format!("{cpath}.severity must be required or recommended."));
                }
            }
        }
        Some(_) => errors.push(format!("{path_label}.criteria must be an array.")),
    }
}

/// `report_shape_errors` is upstream's `else if` guard (`acceptance.ts:250-252`): `false`
/// suppresses the generic `.verify must be an array.` message because the caller already
/// emitted the more specific `verified`-needs-commands one.
fn validate_verify_input(
    errors: &mut Vec<String>,
    verify: Option<&Value>,
    path_label: &str,
    report_shape_errors: bool,
) {
    match verify {
        Option::None => {}
        Some(Value::Array(items)) => {
            for (index, command) in items.iter().enumerate() {
                let vpath = format!("{path_label}.verify[{index}]");
                let Value::Object(cmd) = command else {
                    errors.push(format!("{vpath} must be an object."));
                    continue;
                };
                for key in cmd.keys() {
                    if !ACCEPTANCE_VERIFY_KEYS.contains(&key.as_str()) {
                        errors.push(format!("{vpath}.{key} is not supported."));
                    }
                }
                if cmd
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|i| i.trim().is_empty())
                {
                    errors.push(format!("{vpath}.id is required."));
                }
                if cmd
                    .get("command")
                    .and_then(Value::as_str)
                    .is_none_or(|c| c.trim().is_empty())
                {
                    errors.push(format!("{vpath}.command is required."));
                }
                if let Some(timeout) = cmd.get("timeoutMs") {
                    let ok = timeout
                        .as_i64()
                        .filter(|_| timeout.as_f64().is_some_and(|f| f.fract() == 0.0))
                        .is_some_and(|v| v >= 1);
                    if !ok {
                        errors.push(format!("{vpath}.timeoutMs must be an integer >= 1."));
                    }
                }
                if let Some(cwd) = cmd.get("cwd")
                    && !cwd.is_string()
                {
                    errors.push(format!("{vpath}.cwd must be a string."));
                }
                match cmd.get("env") {
                    Some(Value::Object(env)) => {
                        for (env_key, env_value) in env {
                            if !env_value.is_string() {
                                errors.push(format!("{vpath}.env.{env_key} must be a string."));
                            }
                        }
                    }
                    Some(_) => errors.push(format!("{vpath}.env must be an object.")),
                    Option::None => {}
                }
                if let Some(allow) = cmd.get("allowFailure")
                    && !allow.is_boolean()
                {
                    errors.push(format!("{vpath}.allowFailure must be a boolean."));
                }
            }
        }
        Some(_) if report_shape_errors => {
            errors.push(format!("{path_label}.verify must be an array."));
        }
        Some(_) => {}
    }
}

fn validate_review_input(errors: &mut Vec<String>, review: Option<&Value>, path_label: &str) {
    match review {
        Option::None | Some(Value::Bool(false)) => {}
        Some(Value::Object(map)) => {
            for key in map.keys() {
                if !ACCEPTANCE_REVIEW_KEYS.contains(&key.as_str()) {
                    errors.push(format!("{path_label}.review.{key} is not supported."));
                }
            }
            if let Some(agent) = map.get("agent")
                && !agent.is_string()
            {
                errors.push(format!("{path_label}.review.agent must be a string."));
            }
            if let Some(focus) = map.get("focus")
                && !focus.is_string()
            {
                errors.push(format!("{path_label}.review.focus must be a string."));
            }
            if let Some(required) = map.get("required")
                && !required.is_boolean()
            {
                errors.push(format!("{path_label}.review.required must be a boolean."));
            }
        }
        Some(_) => errors.push(format!("{path_label}.review must be false or an object.")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use serde_json::json;

    #[test]
    fn duplicate_normalized_criterion_ids_are_rejected() {
        // `criterionIds` (`acceptance.ts:206,218-224` @v0.43.0). The two ids below collapse onto
        // the SAME `normalizedToken`, which is the key `checkCriteriaSatisfied` matches the
        // child's reported criteria against — so the second gate would silently shadow the
        // first and never be enforced.
        let errors = validate_acceptance_input(
            &json!({"criteria": [
                {"id": "No Staged Files", "must": "a"},
                {"id": "no_staged__files", "must": "b"}
            ]}),
            "acceptance",
        );
        assert_eq!(
            errors,
            vec![
                "acceptance.criteria[1].id duplicates normalized criterion id \
                     'no-staged-files'."
                    .to_string()
            ]
        );

        // Distinct ids are fine, and the set is scoped to ONE `criteria[]` array.
        assert!(
            validate_acceptance_input(
                &json!({"criteria": [{"id": "a", "must": "x"}, {"id": "b", "must": "y"}]}),
                "acceptance"
            )
            .is_empty()
        );

        // A blank id reports "id is required" ONLY — it must not additionally claim to
        // duplicate the empty token (`acceptance.ts:218-224`'s if/else).
        assert_eq!(
            validate_acceptance_input(
                &json!({"criteria": [{"id": "  ", "must": "x"}, {"id": "", "must": "y"}]}),
                "acceptance"
            ),
            vec![
                "acceptance.criteria[0].id is required.".to_string(),
                "acceptance.criteria[1].id is required.".to_string(),
            ]
        );
    }

    // ---- validateAcceptanceInput ----

    #[test]
    fn validates_invalid_disable_and_verify_shapes() {
        let v = |value: Value| validate_acceptance_input(&value, "acceptance");
        assert_eq!(
            v(json!({"level": "none"})),
            vec!["acceptance.reason is required when level is none.".to_string()]
        );
        assert_eq!(
            v(json!({"verify": [{"id": "missing-command"}]})),
            vec!["acceptance.verify[0].command is required.".to_string()]
        );
        assert_eq!(
            v(json!({"verify": [{"id": "fractional", "command": "npm test", "timeoutMs": 1.5}]})),
            vec!["acceptance.verify[0].timeoutMs must be an integer >= 1.".to_string()]
        );
        assert!(v(json!(false)).is_empty());
        assert!(v(json!("checked")).is_empty());
        assert!(
            v(json!({"criteria": ["ship the fix"], "review": false, "stopRules": ["stay scoped"]}))
                .is_empty()
        );
        assert!(
            v(json!({"criteria": [{"id": "missing-must"}]}))
                .iter()
                .any(|e| e.contains("acceptance.criteria[0].must is required"))
        );
        assert!(
            v(json!({"criteria": [123]}))
                .iter()
                .any(|e| e.contains("acceptance.criteria[0] must be a string or an object"))
        );
        // v0.43.0 routes this through `unsupportedEvidenceKindMessage` (`acceptance.ts:171-174`),
        // which ECHOES the offending value and appends `ACCEPTANCE_EVIDENCE_HELP` (`:48`).
        // Asserted whole rather than by substring so the guidance cannot silently regress.
        assert_eq!(
            v(json!({"evidence": ["bogus"]})),
            vec![
                "acceptance.evidence[0] \"bogus\" is not a supported evidence kind. Supported \
                     evidence kinds: changed-files, tests-added, commands-run, validation-output, \
                     residual-risks, no-staged-files, diff-summary, review-findings, manual-notes. \
                     Example: { level: \"checked\", evidence: [\"commands-run\", \"changed-files\"] }."
                    .to_string()
            ]
        );
        // A NON-string entry gets the same message with no echo (`typeof item === "string" ?
        // ` "${item}"` : ""`, `acceptance.ts:172`).
        assert!(v(json!({"evidence": [7]})).iter().any(|e| e.starts_with(
            "acceptance.evidence[0] is not a supported evidence kind. Supported evidence kinds: "
        )));
        assert!(
            v(json!({"review": true}))
                .iter()
                .any(|e| e.contains("acceptance.review must be false or an object"))
        );
        assert!(
            v(json!({"review": {"required": "yes"}}))
                .iter()
                .any(|e| e.contains("acceptance.review.required must be a boolean"))
        );
        assert!(
            v(json!({"stopRules": [123]}))
                .iter()
                .any(|e| e.contains("acceptance.stopRules[0] must be a string"))
        );
        assert!(
            v(json!({"surprise": true}))
                .iter()
                .any(|e| e.contains("acceptance.surprise is not supported"))
        );
    }
}
