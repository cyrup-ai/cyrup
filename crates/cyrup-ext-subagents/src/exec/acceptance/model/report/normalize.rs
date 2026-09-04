//! G79 report normalization (pi `acceptance.ts:484-628` @v0.43.0): coercing a child's
//! loosely-shaped report value onto the canonical field names and status tokens.

use serde_json::Value;

use super::super::report::validate::path_for;

// --------------------------------------------------------------------------------------------
// G79: report normalization (acceptance.ts:484-628 @v0.43.0)
//
// v0.34.0 had only `unwrapAcceptanceReport` + `validationPathLabelForWrapper`: two wrapper keys,
// no aliases, no coercions, and no errors for anything unexpected. v0.43.0 replaced both with a
// single normalization pass that runs INSIDE `validateAcceptanceReport`, so every entry point
// (explicit fence, generic JSON fence, `ACCEPTANCE_REPORT:` marker, caller-supplied report)
// gets the same treatment.
// --------------------------------------------------------------------------------------------

/// `ACCEPTANCE_REPORT_WRAPPERS` (acceptance.ts:484) — four spellings, up from v0.34.0's two.
const ACCEPTANCE_REPORT_WRAPPERS: &[&str] = &[
    "acceptance",
    "acceptance-report",
    "acceptance_report",
    "acceptanceReport",
];

/// `ACCEPTANCE_REPORT_FIELDS` (acceptance.ts:486-508) — `(wire key, canonical key)`. Every
/// camelCase field also answers to its snake_case spelling, and `notes` is canonical on its own.
const ACCEPTANCE_REPORT_FIELDS: &[(&str, &str)] = &[
    ("criteriaSatisfied", "criteriaSatisfied"),
    ("criteria_satisfied", "criteriaSatisfied"),
    ("changedFiles", "changedFiles"),
    ("changed_files", "changedFiles"),
    ("testsAddedOrUpdated", "testsAddedOrUpdated"),
    ("tests_added_or_updated", "testsAddedOrUpdated"),
    ("commandsRun", "commandsRun"),
    ("commands_run", "commandsRun"),
    ("validationOutput", "validationOutput"),
    ("validation_output", "validationOutput"),
    ("residualRisks", "residualRisks"),
    ("residual_risks", "residualRisks"),
    ("noStagedFiles", "noStagedFiles"),
    ("no_staged_files", "noStagedFiles"),
    ("diffSummary", "diffSummary"),
    ("diff_summary", "diffSummary"),
    ("reviewFindings", "reviewFindings"),
    ("review_findings", "reviewFindings"),
    ("manualNotes", "manualNotes"),
    ("manual_notes", "manualNotes"),
    ("notes", "notes"),
];

/// `CRITERION_REPORT_FIELDS` (acceptance.ts:510).
const CRITERION_REPORT_FIELDS: &[&str] = &["id", "status", "evidence"];
/// `COMMAND_REPORT_FIELDS` (acceptance.ts:511).
const COMMAND_REPORT_FIELDS: &[&str] = &["command", "result", "summary"];

/// `normalizedToken` (acceptance.ts:513-515):
/// `value.trim().toLowerCase().replace(/[\s_]+/g, "-").replace(/-+/g, "-")`.
///
/// Both replacements are reproduced literally, in order, because they are not the same as one
/// combined `[\s_-]+ -> "-"` pass at the edges: `trim()` has already removed surrounding
/// whitespace, so a leading `_` or `-` survives as a leading `-` rather than being dropped.
pub(crate) fn normalized_token(value: &str) -> String {
    let lowered = value.trim().to_lowercase();
    // `/[\s_]+/g -> "-"`
    let mut collapsed = String::with_capacity(lowered.len());
    let mut in_run = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() || ch == '_' {
            if !in_run {
                collapsed.push('-');
                in_run = true;
            }
        } else {
            in_run = false;
            collapsed.push(ch);
        }
    }
    // `/-+/g -> "-"`
    let mut out = String::with_capacity(collapsed.len());
    let mut prev_dash = false;
    for ch in collapsed.chars() {
        if ch == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            prev_dash = false;
            out.push(ch);
        }
    }
    out
}

/// `normalizeCriterionStatus` (acceptance.ts:517-524) — the alias table a model actually emits
/// (`"Done"`, `"PASSED"`, `"n/a"`-style tokens) folded onto the three canonical statuses. A
/// value that matches nothing is returned UNCHANGED so `validateAcceptanceReport` still reports
/// it with its original text.
fn normalize_criterion_status(value: &Value) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let token = normalized_token(text);
    match token.as_str() {
        "satisfied" | "met" | "complete" | "completed" | "done" | "pass" | "passed" | "success"
        | "succeeded" => Value::String("satisfied".to_string()),
        "not-satisfied" | "not-met" | "unmet" | "incomplete" | "fail" | "failed" => {
            Value::String("not-satisfied".to_string())
        }
        "not-applicable" | "n-a" | "na" | "skip" | "skipped" => {
            Value::String("not-applicable".to_string())
        }
        _ => value.clone(),
    }
}

/// `normalizeCommandResult` (acceptance.ts:526-533).
fn normalize_command_result(value: &Value) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let token = normalized_token(text);
    match token.as_str() {
        "passed" | "pass" | "success" | "successful" | "succeeded" | "ok" => {
            Value::String("passed".to_string())
        }
        "failed" | "fail" | "failure" | "error" => Value::String("failed".to_string()),
        "not-run" | "not-executed" | "skip" | "skipped" => Value::String("not-run".to_string()),
        _ => value.clone(),
    }
}

/// `normalizeCriterionReport` (acceptance.ts:535-550).
fn normalize_criterion_report(value: &Value, path_label: &str, errors: &mut Vec<String>) -> Value {
    let Value::Object(map) = value else {
        return value.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, field) in map {
        if !CRITERION_REPORT_FIELDS.contains(&key.as_str()) {
            errors.push(format!(
                "{path_label}.{key}: unsupported acceptance criterion field"
            ));
            continue;
        }
        let normalized = match key.as_str() {
            "id" => match field.as_str() {
                Some(text) => Value::String(normalized_token(text)),
                Option::None => field.clone(),
            },
            "status" => normalize_criterion_status(field),
            _ => field.clone(),
        };
        out.insert(key.clone(), normalized);
    }
    Value::Object(out)
}

/// `normalizeCommandReport` (acceptance.ts:552-563).
fn normalize_command_report(value: &Value, path_label: &str, errors: &mut Vec<String>) -> Value {
    let Value::Object(map) = value else {
        return value.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, field) in map {
        if !COMMAND_REPORT_FIELDS.contains(&key.as_str()) {
            errors.push(format!(
                "{path_label}.{key}: unsupported acceptance command field"
            ));
            continue;
        }
        let normalized = if key == "result" {
            normalize_command_result(field)
        } else {
            field.clone()
        };
        out.insert(key.clone(), normalized);
    }
    Value::Object(out)
}

/// The outcome of [`normalize_acceptance_report_value`].
pub(crate) struct NormalizedReportValue {
    pub(crate) value: Value,
    pub(crate) path_label: String,
    pub(crate) errors: Vec<String>,
}

/// `normalizeAcceptanceReportValue` (acceptance.ts:565-628): unwrap an acceptance wrapper key,
/// fold every field alias onto its canonical name, and coerce the shapes a model reliably gets
/// wrong — a lone object where an array belongs, a bare string where a `string[]` belongs, and
/// `"true"`/`"false"` where `noStagedFiles` wants a boolean.
///
/// **[CYRUP-DELTA]** `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature is
/// off workspace-wide), so key iteration is alphabetical rather than insertion-ordered. That
/// changes only two things against upstream and neither is a behavioural gate: which of several
/// simultaneously-present wrapper keys is picked as `wrapperKey` (upstream picks the first
/// authored, this picks the alphabetically first — and either way the ambiguity itself is
/// reported), and the ORDER of the accumulated error strings.
pub(crate) fn normalize_acceptance_report_value(
    value: &Value,
    path_label: &str,
) -> NormalizedReportValue {
    let mut errors: Vec<String> = Vec::new();
    let mut report_value = value.clone();
    let mut report_path = path_label.to_string();

    if let Value::Object(map) = &report_value {
        let wrapper_keys: Vec<&String> = map
            .keys()
            .filter(|key| ACCEPTANCE_REPORT_WRAPPERS.contains(&key.as_str()))
            .collect();
        if let Some(wrapper_key) = wrapper_keys.first().map(|key| (*key).clone()) {
            if wrapper_keys.len() > 1 {
                let label = if path_label.is_empty() {
                    "acceptance-report"
                } else {
                    path_label
                };
                errors.push(format!(
                    "{label}: multiple acceptance report wrappers are ambiguous"
                ));
            }
            for key in map.keys() {
                if key != &wrapper_key {
                    errors.push(format!(
                        "{}: unsupported alongside acceptance report wrapper '{wrapper_key}'",
                        path_for(path_label, key)
                    ));
                }
            }
            report_path = path_for(path_label, &wrapper_key);
            report_value = map.get(&wrapper_key).cloned().unwrap_or(Value::Null);
        }
    }

    let Value::Object(map) = &report_value else {
        return NormalizedReportValue {
            value: report_value,
            path_label: report_path,
            errors,
        };
    };

    let mut normalized = serde_json::Map::new();
    for (key, field) in map {
        let Some((_, canonical)) = ACCEPTANCE_REPORT_FIELDS
            .iter()
            .find(|(wire, _)| *wire == key.as_str())
        else {
            errors.push(format!(
                "{}: unsupported acceptance report field",
                path_for(&report_path, key)
            ));
            continue;
        };
        if normalized.contains_key(*canonical) {
            errors.push(format!(
                "{}: duplicates normalized field '{canonical}'",
                path_for(&report_path, key)
            ));
            continue;
        }
        let field_path = path_for(&report_path, canonical);
        let normalized_field = match *canonical {
            "criteriaSatisfied" | "commandsRun" => {
                // A lone object is read as a one-element array (`acceptance.ts:598,605`).
                let items: Option<Vec<Value>> = match field {
                    Value::Array(items) => Some(items.clone()),
                    Value::Object(_) => Some(vec![field.clone()]),
                    _ => Option::None,
                };
                match items {
                    Some(items) => Value::Array(
                        items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| {
                                let item_path = format!("{field_path}[{index}]");
                                if *canonical == "criteriaSatisfied" {
                                    normalize_criterion_report(item, &item_path, &mut errors)
                                } else {
                                    normalize_command_report(item, &item_path, &mut errors)
                                }
                            })
                            .collect(),
                    ),
                    Option::None => field.clone(),
                }
            }
            "changedFiles"
            | "testsAddedOrUpdated"
            | "validationOutput"
            | "residualRisks"
            | "reviewFindings" => match field {
                Value::String(_) => Value::Array(vec![field.clone()]),
                _ => field.clone(),
            },
            "noStagedFiles" => match field.as_str().map(|s| s.trim().to_lowercase()) {
                Some(token) if token == "true" => Value::Bool(true),
                Some(token) if token == "false" => Value::Bool(false),
                _ => field.clone(),
            },
            _ => field.clone(),
        };
        normalized.insert((*canonical).to_string(), normalized_field);
    }

    NormalizedReportValue {
        value: Value::Object(normalized),
        path_label: report_path,
        errors,
    }
}
