//! `validateAcceptanceReport` (pi `acceptance.ts:517-620`) — structural validation with exact
//! error-message parity against upstream.

use serde_json::Value;

use super::super::report::normalize::normalize_acceptance_report_value;
use super::super::types::AcceptanceReport;

// --------------------------------------------------------------------------------------------
// validateAcceptanceReport (acceptance.ts:517-620) — exact error-message parity
// --------------------------------------------------------------------------------------------

fn describe_validation_value(value: Option<&Value>) -> String {
    match value {
        Option::None => "missing".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Array(_)) => "array".to_string(),
        Some(Value::Object(_)) => "object".to_string(),
        Some(Value::String(s)) => {
            let short: String = if s.chars().count() > 80 {
                let head: String = s.chars().take(77).collect();
                format!("{head}...")
            } else {
                s.clone()
            };
            serde_json::to_string(&Value::String(short)).unwrap_or_else(|_| format!("\"{s}\""))
        }
        Some(Value::Number(n)) => format!("number {n}"),
        Some(Value::Bool(b)) => format!("boolean {b}"),
    }
}

fn push_type_error(errors: &mut Vec<String>, path: &str, expected: &str, value: Option<&Value>) {
    errors.push(format!(
        "{path}: expected {expected}; got {}",
        describe_validation_value(value)
    ));
}

pub(crate) fn path_for(base: &str, segment: &str) -> String {
    if base.is_empty() {
        segment.to_string()
    } else {
        format!("{base}.{segment}")
    }
}

fn validate_string_array_field(errors: &mut Vec<String>, value: Option<&Value>, path: &str) {
    let Some(Value::Array(items)) = value else {
        push_type_error(errors, path, "string[]", value);
        return;
    };
    for (index, item) in items.iter().enumerate() {
        // `acceptance.ts:827` @v0.43.0 — a blank entry is no longer accepted as evidence
        // (v0.34.0 only required `typeof item === "string"`).
        if !item.as_str().is_some_and(|s| !s.trim().is_empty()) {
            push_type_error(
                errors,
                &format!("{path}[{index}]"),
                "non-empty string",
                Some(item),
            );
        }
    }
}

/// `validateAcceptanceReport` (acceptance.ts:831-909). Returns `(report, errors)`; a present
/// `report` requires no errors AND at least one recognized report field.
pub(crate) fn validate_acceptance_report(
    value: &Value,
    path_label: &str,
) -> (Option<AcceptanceReport>, Vec<String>) {
    // `acceptance.ts:831-835` @v0.43.0 — normalization runs FIRST and its errors seed the list,
    // so an unsupported alias or a duplicated field is reported alongside the type errors
    // rather than silently dropping the whole report.
    let normalized = normalize_acceptance_report_value(value, path_label);
    let value = &normalized.value;
    let path_label = normalized.path_label.as_str();
    let mut errors: Vec<String> = normalized.errors;
    let Value::Object(map) = value else {
        let label = if path_label.is_empty() {
            "acceptance-report"
        } else {
            path_label
        };
        push_type_error(&mut errors, label, "object", Some(value));
        return (Option::None, errors);
    };

    // criteriaSatisfied
    if let Some(criteria) = map.get("criteriaSatisfied") {
        let cpath = path_for(path_label, "criteriaSatisfied");
        if let Value::Array(items) = criteria {
            // `acceptance.ts:845,855-858`: ids are compared AFTER `normalizedToken`, so
            // `"C 1"` and `"c_1"` are the same criterion and the second one is a duplicate.
            let mut criterion_ids: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for (index, item) in items.iter().enumerate() {
                let ipath = format!("{cpath}[{index}]");
                let Value::Object(obj) = item else {
                    push_type_error(&mut errors, &ipath, "object", Some(item));
                    continue;
                };
                match obj.get("id") {
                    Some(id) if !id.is_string() => {
                        push_type_error(&mut errors, &format!("{ipath}.id"), "string", Some(id));
                    }
                    Some(Value::String(id))
                        if !id.is_empty() && !criterion_ids.insert(id.as_str()) =>
                    {
                        errors.push(format!(
                            "{ipath}.id: duplicate normalized criterion id '{id}'"
                        ));
                    }
                    _ => {}
                }
                let status = obj.get("status").and_then(Value::as_str);
                if !matches!(
                    status,
                    Some("satisfied") | Some("not-satisfied") | Some("not-applicable")
                ) {
                    push_type_error(
                        &mut errors,
                        &format!("{ipath}.status"),
                        "one of \"satisfied\", \"not-satisfied\", \"not-applicable\"",
                        obj.get("status"),
                    );
                }
                let evidence_ok = obj
                    .get("evidence")
                    .and_then(Value::as_str)
                    .is_some_and(|e| !e.trim().is_empty());
                if !evidence_ok {
                    push_type_error(
                        &mut errors,
                        &format!("{ipath}.evidence"),
                        "non-empty string",
                        obj.get("evidence"),
                    );
                }
            }
        } else {
            push_type_error(&mut errors, &cpath, "array", Some(criteria));
        }
    }

    if map.contains_key("changedFiles") {
        validate_string_array_field(
            &mut errors,
            map.get("changedFiles"),
            &path_for(path_label, "changedFiles"),
        );
    }
    if map.contains_key("testsAddedOrUpdated") {
        validate_string_array_field(
            &mut errors,
            map.get("testsAddedOrUpdated"),
            &path_for(path_label, "testsAddedOrUpdated"),
        );
    }
    if let Some(commands) = map.get("commandsRun") {
        let cpath = path_for(path_label, "commandsRun");
        if let Value::Array(items) = commands {
            for (index, item) in items.iter().enumerate() {
                let ipath = format!("{cpath}[{index}]");
                let Value::Object(obj) = item else {
                    push_type_error(&mut errors, &ipath, "object", Some(item));
                    continue;
                };
                let command_ok = obj
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| !c.trim().is_empty());
                if !command_ok {
                    push_type_error(
                        &mut errors,
                        &format!("{ipath}.command"),
                        "non-empty string",
                        obj.get("command"),
                    );
                }
                let result = obj.get("result").and_then(Value::as_str);
                if !matches!(result, Some("passed") | Some("failed") | Some("not-run")) {
                    push_type_error(
                        &mut errors,
                        &format!("{ipath}.result"),
                        "one of \"passed\", \"failed\", \"not-run\"",
                        obj.get("result"),
                    );
                }
                // `acceptance.ts:883` @v0.43.0 — non-empty (v0.34.0 accepted `""`).
                if !obj
                    .get("summary")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    push_type_error(
                        &mut errors,
                        &format!("{ipath}.summary"),
                        "non-empty string",
                        obj.get("summary"),
                    );
                }
            }
        } else {
            push_type_error(&mut errors, &cpath, "array", Some(commands));
        }
    }
    if map.contains_key("validationOutput") {
        validate_string_array_field(
            &mut errors,
            map.get("validationOutput"),
            &path_for(path_label, "validationOutput"),
        );
    }
    if map.contains_key("residualRisks") {
        validate_string_array_field(
            &mut errors,
            map.get("residualRisks"),
            &path_for(path_label, "residualRisks"),
        );
    }
    if let Some(no_staged) = map.get("noStagedFiles")
        && !no_staged.is_boolean()
    {
        push_type_error(
            &mut errors,
            &path_for(path_label, "noStagedFiles"),
            "boolean",
            Some(no_staged),
        );
    }
    // `acceptance.ts:890` @v0.43.0 — non-empty (v0.34.0 accepted `""`).
    if let Some(diff) = map.get("diffSummary")
        && !diff.as_str().is_some_and(|s| !s.trim().is_empty())
    {
        push_type_error(
            &mut errors,
            &path_for(path_label, "diffSummary"),
            "non-empty string",
            Some(diff),
        );
    }
    if map.contains_key("reviewFindings") {
        validate_string_array_field(
            &mut errors,
            map.get("reviewFindings"),
            &path_for(path_label, "reviewFindings"),
        );
    }
    if let Some(notes) = map.get("manualNotes")
        && !notes.is_string()
    {
        push_type_error(
            &mut errors,
            &path_for(path_label, "manualNotes"),
            "string",
            Some(notes),
        );
    }
    if let Some(notes) = map.get("notes")
        && !notes.is_string()
    {
        push_type_error(
            &mut errors,
            &path_for(path_label, "notes"),
            "string",
            Some(notes),
        );
    }

    if !errors.is_empty() {
        return (Option::None, errors);
    }
    let has_field = [
        "criteriaSatisfied",
        "changedFiles",
        "testsAddedOrUpdated",
        "commandsRun",
        "validationOutput",
        "residualRisks",
        "noStagedFiles",
        "diffSummary",
        "manualNotes",
        "notes",
        "reviewFindings",
    ]
    .iter()
    .any(|key| map.contains_key(*key));
    if !has_field {
        let label = if path_label.is_empty() {
            "acceptance-report"
        } else {
            path_label
        };
        return (
            Option::None,
            vec![format!(
                "{label}: expected at least one acceptance report field"
            )],
        );
    }
    // Validation passed: deserialize the known fields into the typed report (unknown fields are
    // ignored, matching pi's tolerant read-back).
    match serde_json::from_value::<AcceptanceReport>(value.clone()) {
        Ok(report) => (Some(report), errors),
        Err(err) => (Option::None, vec![err.to_string()]),
    }
}

/// `hasGenericAcceptanceReportSignal` (acceptance.ts:630-644 @v0.43.0) — plain KEY PRESENCE on
/// the NORMALIZED value.
///
/// v0.34.0 additionally type-checked each companion (`isStringArray(record.changedFiles)`, …),
/// which meant a `json`-fenced report whose `changedFiles` was a bare string — a shape v0.43.0's
/// normalizer now repairs — read as "not a report at all" and was silently left in the
/// delivered output. Presence alone is the signal; the shape is then `validateAcceptanceReport`'s
/// business and its errors are surfaced rather than swallowed.
pub(crate) fn has_generic_acceptance_report_signal(value: &Value) -> bool {
    let Value::Object(map) = value else {
        return false;
    };
    if !map.contains_key("criteriaSatisfied") {
        return false;
    }
    [
        "changedFiles",
        "testsAddedOrUpdated",
        "commandsRun",
        "validationOutput",
        "residualRisks",
        "noStagedFiles",
        "diffSummary",
        "reviewFindings",
        "manualNotes",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}
