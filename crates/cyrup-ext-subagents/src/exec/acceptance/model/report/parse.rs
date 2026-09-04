//! `parseAcceptanceReport`/`stripAcceptanceReport` (pi `acceptance.ts:437-515`): recovering a
//! report from a child's answer text, and removing it once recovered.

use serde_json::Value;

use super::super::report::fences::{
    extract_balanced_json, fenced_block_bodies, fenced_matches, parse_report_json,
};
use super::super::report::normalize::normalize_acceptance_report_value;
use super::super::report::validate::{
    has_generic_acceptance_report_signal, validate_acceptance_report,
};
use super::super::types::AcceptanceReport;

// --------------------------------------------------------------------------------------------
// parseAcceptanceReport / stripAcceptanceReport (acceptance.ts:437-515)
// --------------------------------------------------------------------------------------------

/// `parseAcceptanceReport` result (acceptance.ts:701).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedAcceptanceReport {
    pub report: Option<AcceptanceReport>,
    pub error: Option<String>,
}

/// `ACCEPTANCE_REPORT_NOT_FOUND` (acceptance.ts:699) — the ONE error string that means "there
/// was simply no report here", as opposed to "there was one and it was broken". The source
/// fallback in [`parse_acceptance_report_sources`] switches on exactly this value, so it is a
/// named constant rather than a repeated literal.
pub const ACCEPTANCE_REPORT_NOT_FOUND: &str = "Structured acceptance report not found.";

/// The tag alternation `` ```acceptance[-_]report `` (acceptance.ts:702-703) — v0.43.0 accepts
/// the underscore spelling everywhere the hyphenated one is accepted.
const ACCEPTANCE_REPORT_FENCE_TAGS: &[&str] = &["acceptance-report", "acceptance_report"];

/// `parseAcceptanceReportBody` (acceptance.ts:666-668).
fn parse_acceptance_report_body(
    body: &str,
) -> Result<(Option<AcceptanceReport>, Vec<String>), String> {
    let parsed = parse_report_json(body)?;
    Ok(validate_acceptance_report(&parsed, ""))
}

/// `parseUnterminatedAcceptanceReportFence` (acceptance.ts:670-683) — recovery for the single
/// most common malformed emission: the model opened ```` ```acceptance-report ```` and then ran
/// out of turn without ever closing the fence. Everything after the opener is parsed as the
/// body. Only attempted when NO closing ``` follows the opener at all, so a well-formed run is
/// never re-parsed by this path.
///
/// Returns `(report, error)`; `(None, None)` means "this recovery does not apply".
fn parse_unterminated_acceptance_report_fence(
    output: &str,
) -> (Option<AcceptanceReport>, Option<String>) {
    let Some((body_start, _)) = find_acceptance_report_fence_opener(output) else {
        return (Option::None, Option::None);
    };
    if output
        .get(body_start..)
        .is_some_and(|rest| rest.contains("```"))
    {
        return (Option::None, Option::None);
    }
    let body = output.get(body_start..).unwrap_or("").trim();
    match serde_json::from_str::<Value>(body) {
        Ok(parsed) => {
            let (report, errors) = validate_acceptance_report(&parsed, "");
            match report {
                Some(report) => (Some(report), Option::None),
                Option::None => (
                    Option::None,
                    Some(format!(
                        "Failed to parse acceptance-report: Invalid acceptance-report: {}",
                        errors.join("; ")
                    )),
                ),
            }
        }
        Err(err) => (
            Option::None,
            Some(format!("Failed to parse acceptance-report: {err}")),
        ),
    }
}

/// `/```acceptance[-_]report\b/i.test(output)` (acceptance.ts:702) — pure TAG PRESENCE, with a
/// `\b` after the tag and NO requirement that a newline follow it.
///
/// This is deliberately NOT [`find_acceptance_report_fence_opener`], whose upstream twin
/// (`parseUnterminatedAcceptanceReportFence`, acceptance.ts:671) really does anchor on
/// `[^\n]*\n` because it needs the offset where the fence BODY starts. `parseAcceptanceReport`
/// only needs to know a fence was OPENED. Conflating the two loses exactly the case where a
/// model was cut off mid-opener (`"…\n```acceptance-report"` with nothing after it): the run
/// then reports [`ACCEPTANCE_REPORT_NOT_FOUND`] instead of the fence defect, and because that
/// constant is the one value [`parse_acceptance_report_sources`] and
/// [`crate::exec::acceptance::lattice::report_source::select_acceptance_report_source`] branch on to decide "genuinely absent, fall
/// through to the other source", a truncated report in a `file-only` artifact papers itself
/// over with the assistant text — the precise failure this source-selection rule exists to
/// prevent.
fn has_acceptance_report_fence_tag(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lowered.get(from..).and_then(|s| s.find("```")) {
        let fence_at = from + rel;
        let after_fence = fence_at + 3;
        let rest = lowered.get(after_fence..).unwrap_or("");
        if let Some(tag) = ACCEPTANCE_REPORT_FENCE_TAGS
            .iter()
            .find(|tag| rest.starts_with(**tag))
        {
            let after_tag = after_fence + tag.len();
            // `\b`: the tag must not run straight into another word character.
            let boundary_ok = lowered
                .get(after_tag..)
                .and_then(|s| s.chars().next())
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
            if boundary_ok {
                return true;
            }
        }
        from = after_fence;
    }
    false
}

/// `/```acceptance[-_]report\b[^\n]*\n/gi.exec(output)` (acceptance.ts:671-673): the byte offset
/// just past the opener's newline, plus the offset of the opener itself.
fn find_acceptance_report_fence_opener(output: &str) -> Option<(usize, usize)> {
    let lowered = output.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lowered.get(from..).and_then(|s| s.find("```")) {
        let fence_at = from + rel;
        let after_fence = fence_at + 3;
        let rest = lowered.get(after_fence..).unwrap_or("");
        let tag = ACCEPTANCE_REPORT_FENCE_TAGS
            .iter()
            .find(|tag| rest.starts_with(**tag));
        if let Some(tag) = tag {
            let after_tag = after_fence + tag.len();
            // `\b`: the tag must not run straight into another word character.
            let boundary_ok = lowered
                .get(after_tag..)
                .and_then(|s| s.chars().next())
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
            if boundary_ok && let Some(nl_rel) = lowered.get(after_tag..).and_then(|s| s.find('\n'))
            {
                return Some((after_tag + nl_rel + 1, fence_at));
            }
        }
        from = after_fence;
    }
    Option::None
}

/// `parseGenericJsonAcceptanceReportBody` (acceptance.ts:685-697).
///
/// Returns `(report, error)`. v0.34.0 returned only `Option<AcceptanceReport>` and swallowed
/// every validation failure, so a `json`-fenced block that was unmistakably a report but had one
/// bad field was treated as unrelated prose. v0.43.0 surfaces those errors — but ONLY when the
/// value still carries the `criteriaSatisfied` marker, so genuinely unrelated JSON stays quiet.
fn parse_generic_json_acceptance_report_body(
    body: &str,
) -> Result<(Option<AcceptanceReport>, Option<String>), String> {
    let parsed = parse_report_json(body)?;
    let normalized = normalize_acceptance_report_value(&parsed, "");
    let has_criteria_marker = matches!(
        &normalized.value,
        Value::Object(map) if map.contains_key("criteriaSatisfied")
    );
    if !has_generic_acceptance_report_signal(&normalized.value)
        && !(has_criteria_marker && !normalized.errors.is_empty())
    {
        return Ok((Option::None, Option::None));
    }
    let (report, errors) = validate_acceptance_report(&parsed, "");
    Ok(match report {
        Some(report) => (Some(report), Option::None),
        Option::None => (
            Option::None,
            Some(format!("Invalid acceptance-report: {}", errors.join("; "))),
        ),
    })
}

/// `parseAcceptanceReport` (acceptance.ts:701-751).
#[must_use]
pub fn parse_acceptance_report(output: &str) -> ParsedAcceptanceReport {
    // acceptance.ts:702 tests TAG PRESENCE only — see [`has_acceptance_report_fence_tag`] for
    // why this must not be `find_acceptance_report_fence_opener(...).is_some()`.
    let explicit_fence_present = has_acceptance_report_fence_tag(output);
    let fenced = fenced_block_bodies(output, ACCEPTANCE_REPORT_FENCE_TAGS);
    let mut parse_errors: Vec<String> = Vec::new();
    for body in &fenced {
        match parse_acceptance_report_body(body) {
            Ok((Some(report), _)) => {
                return ParsedAcceptanceReport {
                    report: Some(report),
                    error: Option::None,
                };
            }
            Ok((Option::None, errors)) => {
                parse_errors.push(format!("Invalid acceptance-report: {}", errors.join("; ")));
            }
            Err(message) => parse_errors.push(message),
        }
    }
    if !parse_errors.is_empty() {
        return ParsedAcceptanceReport {
            report: Option::None,
            error: Some(format!(
                "Failed to parse acceptance-report: {}",
                parse_errors.join("; ")
            )),
        };
    }
    // `acceptance.ts:715-719`: an OPENED acceptance-report fence that produced no parseable
    // body never falls through to the generic-JSON or marker paths — it is a defect of THIS
    // report, so it is either recovered or reported.
    if explicit_fence_present {
        let (report, error) = parse_unterminated_acceptance_report_fence(output);
        if report.is_some() || error.is_some() {
            return ParsedAcceptanceReport { report, error };
        }
        return ParsedAcceptanceReport {
            report: Option::None,
            error: Some(
                "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
                    .to_string(),
            ),
        };
    }
    for body in fenced_block_bodies(output, &["json", "jsonc", "json5"]) {
        match parse_generic_json_acceptance_report_body(&body) {
            Ok((Some(report), _)) => {
                return ParsedAcceptanceReport {
                    report: Some(report),
                    error: Option::None,
                };
            }
            Ok((Option::None, Some(error))) => {
                return ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(format!("Failed to parse acceptance-report: {error}")),
                };
            }
            // Unrelated JSON, or malformed JSON that is not report-shaped: ignored, exactly as
            // upstream's bare `catch {}` does (`acceptance.ts:725-728`).
            Ok((Option::None, Option::None)) | Err(_) => {}
        }
    }
    // ACCEPTANCE_REPORT: marker (acceptance.ts:730-749). v0.43.0 gives the two "the marker is
    // there but the object is not" cases their own messages instead of silently falling through
    // to `ACCEPTANCE_REPORT_NOT_FOUND`.
    if let Some(marker_index) = find_acceptance_report_marker(output) {
        let Some(json_start) = output
            .get(marker_index..)
            .and_then(|s| s.find('{'))
            .map(|r| marker_index + r)
        else {
            return ParsedAcceptanceReport {
                report: Option::None,
                error: Some(
                    "Failed to parse acceptance-report: Expected a JSON object after ACCEPTANCE_REPORT:."
                        .to_string(),
                ),
            };
        };
        let Some(json) = extract_balanced_json(output, json_start) else {
            return ParsedAcceptanceReport {
                report: Option::None,
                error: Some(
                    "Failed to parse acceptance-report: Unterminated JSON object after ACCEPTANCE_REPORT:."
                        .to_string(),
                ),
            };
        };
        return match serde_json::from_str::<Value>(&json) {
            Ok(parsed) => {
                let (validated, errors) = validate_acceptance_report(&parsed, "");
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
            Err(err) => ParsedAcceptanceReport {
                report: Option::None,
                error: Some(format!("Failed to parse acceptance-report: {err}")),
            },
        };
    }
    ParsedAcceptanceReport {
        report: Option::None,
        error: Some(ACCEPTANCE_REPORT_NOT_FOUND.to_string()),
    }
}

/// `parseAcceptanceReportSources` (acceptance.ts:753-772) — search BOTH the assistant output and
/// the child's configured output file, in the order `authoritative` dictates.
///
/// The load-bearing rule is the fallthrough condition: only a genuinely ABSENT report
/// ([`ACCEPTANCE_REPORT_NOT_FOUND`]) in the primary source falls through to the secondary. A
/// MALFORMED report in the primary source is a defect to surface, not a miss to paper over.
#[must_use]
pub fn parse_acceptance_report_sources(
    output: &str,
    file_output: Option<&crate::exec::acceptance::AcceptanceFileOutput<'_>>,
) -> ParsedAcceptanceReport {
    let from_text = || parse_acceptance_report(output);
    let from_file = || match file_output {
        Option::None => ParsedAcceptanceReport {
            report: Option::None,
            error: Some(ACCEPTANCE_REPORT_NOT_FOUND.to_string()),
        },
        Some(file) => {
            let parsed = parse_acceptance_report(file.content);
            if parsed.report.is_some()
                || parsed.error.as_deref() == Some(ACCEPTANCE_REPORT_NOT_FOUND)
            {
                parsed
            } else {
                ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(format!(
                        "{} (in configured output {})",
                        parsed.error.unwrap_or_default(),
                        file.path.display()
                    )),
                }
            }
        }
    };
    let authoritative = file_output.is_some_and(|file| file.authoritative);
    let first = if authoritative {
        from_file()
    } else {
        from_text()
    };
    if first.report.is_some() || first.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND) {
        return first;
    }
    if authoritative {
        from_text()
    } else {
        from_file()
    }
}

/// Case-insensitive `/ACCEPTANCE_REPORT\s*:/i` locator (acceptance.ts:473).
fn find_acceptance_report_marker(output: &str) -> Option<usize> {
    let upper = output.to_ascii_uppercase();
    let mut from = 0usize;
    while let Some(rel) = upper.get(from..).and_then(|s| s.find("ACCEPTANCE_REPORT")) {
        let at = from + rel;
        let after = at + "ACCEPTANCE_REPORT".len();
        let rest = upper.get(after..).unwrap_or("");
        let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
        if trimmed.starts_with(':') {
            return Some(at);
        }
        from = at + 1;
    }
    Option::None
}

/// `stripAcceptanceReport` (acceptance.ts:774-795). Removes a trailing `acceptance-report` /
/// generic-JSON acceptance-report fence (and a trailing `ACCEPTANCE_REPORT: {...}` marker) from
/// the DELIVERED output, so a caller sees the human answer, never the machine report JSON.
#[must_use]
pub fn strip_acceptance_report(output: &str) -> String {
    // The trailing-fence variant (`\n?```(acceptance[-_]report|json|jsonc|json5)\s*\n([\s\S]*?)```\s*`,
    // acceptance.ts:775) — five tags at v0.43.0, which added the underscore spelling.
    let tags = [
        "acceptance-report",
        "acceptance_report",
        "json",
        "jsonc",
        "json5",
    ];
    let matches = fenced_matches(output, &tags, true, true);
    // The LAST match with only whitespace after it is the trailing fence (acceptance.ts:777-782).
    let trailing = matches.into_iter().rev().find(|m| {
        output
            .get(m.end..)
            .is_none_or(|tail| tail.trim().is_empty())
    });
    if let Some(fence) = trailing {
        if ACCEPTANCE_REPORT_FENCE_TAGS.contains(&fence.tag.as_str()) {
            return output
                .get(..fence.index)
                .unwrap_or("")
                .trim_end()
                .to_string();
        }
        if matches!(
            parse_generic_json_acceptance_report_body(&fence.body),
            Ok((Some(_), _))
        ) {
            return output
                .get(..fence.index)
                .unwrap_or("")
                .trim_end()
                .to_string();
        }
    }
    // Fallbacks (acceptance.ts:511-514): a trailing acceptance-report fence, then a trailing
    // ACCEPTANCE_REPORT: {...} marker, then trimEnd.
    let stripped = strip_trailing_acceptance_report_fence(output);
    let stripped = strip_trailing_acceptance_marker(&stripped);
    stripped.trim_end().to_string()
}

/// `/\n?```acceptance[-_]report\s*\n[\s\S]*?```\s*$/i` (acceptance.ts:792).
fn strip_trailing_acceptance_report_fence(output: &str) -> String {
    let matches = fenced_matches(output, ACCEPTANCE_REPORT_FENCE_TAGS, true, true);
    if let Some(fence) = matches
        .into_iter()
        .rev()
        .find(|m| output.get(m.end..).is_none_or(|tail| tail.is_empty()))
    {
        return output.get(..fence.index).unwrap_or("").to_string();
    }
    output.to_string()
}

/// `/\n?ACCEPTANCE_REPORT\s*:\s*\{[\s\S]*\}\s*$/i` (acceptance.ts:513) — greedy `{...}` to the
/// LAST `}` before trailing whitespace/end.
fn strip_trailing_acceptance_marker(output: &str) -> String {
    let Some(marker_index) = find_acceptance_report_marker(output) else {
        return output.to_string();
    };
    let Some(brace_rel) = output.get(marker_index..).and_then(|s| s.find('{')) else {
        return output.to_string();
    };
    // Between the marker and the `{`, only `\s*:\s*` is allowed.
    let between = output
        .get(marker_index + "ACCEPTANCE_REPORT".len()..marker_index + brace_rel)
        .unwrap_or("");
    let between_ok = {
        let t = between.trim();
        t == ":"
    };
    if !between_ok {
        return output.to_string();
    }
    // Greedy: last `}` such that everything after it is whitespace.
    let Some(last_close) = output.rfind('}') else {
        return output.to_string();
    };
    if output
        .get(last_close + 1..)
        .is_none_or(|tail| tail.trim().is_empty())
        && last_close > marker_index
    {
        let start = if marker_index > 0 && output.as_bytes().get(marker_index - 1) == Some(&b'\n') {
            marker_index - 1
        } else {
            marker_index
        };
        return output.get(..start).unwrap_or("").to_string();
    }
    output.to_string()
}

/// Strip acceptance-report fences from every text part of an assistant message value in place
/// (`stripAcceptanceReportsFromMessages`, execution.ts:219-228, applied at :1713) — used by the delivered-output
/// path so a stored transcript never shows the machine report JSON either.
#[must_use]
pub fn strip_acceptance_report_from_message_text(text: &str) -> String {
    strip_acceptance_report(text)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::testsupport::report_text;
    use crate::exec::acceptance::model::testsupport::report_value;
    use crate::exec::acceptance::model::types::CommandRunResult;
    use crate::exec::acceptance::model::types::CriterionStatus;

    use serde_json::json;

    // ---- parseAcceptanceReport / stripAcceptanceReport ----

    #[test]
    fn parses_acceptance_report_fences_and_ignores_unrelated_json() {
        let parsed = parse_acceptance_report(&report_text(json!({}), "acceptance-report"));
        let report = parsed.report.expect("report");
        assert_eq!(
            report.changed_files.as_deref(),
            Some(&["src/file.ts".to_string()][..])
        );
        assert!(parsed.error.is_none());

        let generic =
            parse_acceptance_report("done\n```json\n{\"notes\":\"not an acceptance report\"}\n```");
        assert!(generic.report.is_none());
        assert!(
            generic
                .error
                .as_deref()
                .unwrap()
                .contains("Structured acceptance report not found")
        );

        let criteria_only = parse_acceptance_report(
            "done\n```json\n{\"criteriaSatisfied\":[{\"id\":\"criterion-1\",\"status\":\"satisfied\",\"evidence\":\"example\"}]}\n```",
        );
        assert!(criteria_only.report.is_none());

        let malformed = parse_acceptance_report("```acceptance-report\n{bad-json\n```");
        assert!(malformed.report.is_none());
        assert!(
            malformed
                .error
                .as_deref()
                .unwrap()
                .contains("Failed to parse acceptance-report")
        );
    }

    #[test]
    fn parses_reports_from_json_family_fences_and_strips_them() {
        for fence in ["json", "jsonc", "json5"] {
            let output = report_text(json!({}), fence);
            let parsed = parse_acceptance_report(&output);
            assert!(parsed.report.is_some(), "fence {fence}");
            assert_eq!(strip_acceptance_report(&output), "done");
        }
    }

    #[test]
    fn strips_trailing_json_report_after_earlier_unrelated_json_fence() {
        let report_json = serde_json::to_string(&report_value(json!({}))).unwrap();
        let output = format!(
            "metadata\n```json\n{}\n```\ndone\n```json\n{}\n```",
            serde_json::to_string(&json!({"notes": "not an acceptance report"})).unwrap(),
            report_json,
        );
        let parsed = parse_acceptance_report(&output);
        assert!(parsed.report.is_some());
        let expected = format!(
            "metadata\n```json\n{}\n```\ndone",
            serde_json::to_string(&json!({"notes": "not an acceptance report"})).unwrap()
        );
        assert_eq!(strip_acceptance_report(&output), expected);
    }

    #[test]
    fn unwraps_acceptance_report_wrapper_objects() {
        let wrapped =
            serde_json::to_string(&json!({"acceptance-report": report_value(json!({}))})).unwrap();
        let output = format!("done\n```json\n{wrapped}\n```");
        let parsed = parse_acceptance_report(&output);
        let report = parsed.report.expect("report");
        assert_eq!(
            report.tests_added_or_updated.as_deref(),
            Some(&["test/file.test.ts".to_string()][..])
        );
        assert_eq!(strip_acceptance_report(&output), "done");
    }

    #[test]
    fn report_shaped_generic_json_without_criteria_is_not_stripped() {
        let output = "done\n```json\n{\"changedFiles\":[\"src/file.ts\"]}\n```".to_string();
        assert!(parse_acceptance_report(&output).report.is_none());
        assert_eq!(strip_acceptance_report(&output), output);
    }

    #[test]
    fn reports_field_level_validation_errors() {
        let bad_review = parse_acceptance_report(&report_text(
            json!({"reviewFindings": [{"id": "B-1", "severity": "blocker", "finding": "Missing evidence"}]}),
            "acceptance-report",
        ));
        assert!(bad_review.report.is_none());
        // G79 — `validateStringArrayField` now demands a NON-EMPTY string (`acceptance.ts:827`).
        assert!(
            bad_review
                .error
                .as_deref()
                .unwrap()
                .contains("reviewFindings[0]: expected non-empty string; got object")
        );

        let bad_command = parse_acceptance_report(&report_text(
            json!({"commandsRun": [{"command": "npm test", "exitCode": 0}]}),
            "acceptance-report",
        ));
        let err = bad_command.error.as_deref().unwrap();
        assert!(err.contains("commandsRun[0].result: expected one of \"passed\", \"failed\", \"not-run\"; got missing"));
        assert!(err.contains("commandsRun[0].summary: expected non-empty string; got missing"));

        // G79 — `"done"` is now a recognized ALIAS of `satisfied` (`acceptance.ts:520`), so the
        // unrecognized-status assertion uses a token that really is unrecognized. The alias
        // itself is asserted separately in
        // `normalizes_status_aliases_field_aliases_and_singleton_shapes`.
        let bad_criteria = parse_acceptance_report(&report_text(
            json!({"criteriaSatisfied": [{"id": 7, "status": "maybe", "evidence": ""}]}),
            "acceptance-report",
        ));
        let err = bad_criteria.error.as_deref().unwrap();
        assert!(err.contains("criteriaSatisfied[0].id: expected string; got number 7"));
        assert!(err.contains("criteriaSatisfied[0].status: expected one of \"satisfied\", \"not-satisfied\", \"not-applicable\"; got \"maybe\""));
        assert!(err.contains("criteriaSatisfied[0].evidence: expected non-empty string; got \"\""));
    }

    // ---- G79: report normalization, recovery and sources (acceptance.ts:484-772) ----

    #[test]
    fn normalizes_status_aliases_field_aliases_and_singleton_shapes() {
        // Every one of these shapes was REJECTED before v0.43.0's normalizer: snake_case keys
        // were "unsupported", a lone object where an array belongs failed the array check, a
        // bare string where a `string[]` belongs failed `string[]`, `"true"` failed the boolean
        // check, and `Done`/`OK` failed the status/result enums.
        let parsed = parse_acceptance_report(
            r#"done
```acceptance-report
{
  "criteria_satisfied": {"id": "C 1", "status": "Done", "evidence": "did it"},
  "changed_files": "src/file.rs",
  "tests_added_or_updated": ["tests/file.rs"],
  "commands_run": {"command": "cargo test", "result": "OK", "summary": "green"},
  "validation_output": "all green",
  "residual_risks": [],
  "no_staged_files": "true",
  "manual_notes": "nothing else"
}
```"#,
        );
        assert_eq!(parsed.error, None);
        let report = parsed.report.expect("the normalized report parses");
        let criterion = &report.criteria_satisfied.as_ref().unwrap()[0];
        assert_eq!(criterion.id.as_deref(), Some("c-1"));
        assert_eq!(criterion.status, CriterionStatus::Satisfied);
        assert_eq!(
            report.changed_files.as_deref(),
            Some(["src/file.rs".to_string()].as_slice())
        );
        assert_eq!(
            report.validation_output.as_deref(),
            Some(["all green".to_string()].as_slice())
        );
        let command = &report.commands_run.as_ref().unwrap()[0];
        assert_eq!(command.result, CommandRunResult::Passed);
        assert_eq!(report.no_staged_files, Some(true));
        assert_eq!(report.manual_notes.as_deref(), Some("nothing else"));
    }

    #[test]
    fn rejects_duplicate_normalized_criterion_ids_and_unsupported_fields() {
        let parsed = parse_acceptance_report(&report_text(
            json!({"criteriaSatisfied": [
                {"id": "c 1", "status": "satisfied", "evidence": "one"},
                {"id": "C_1", "status": "satisfied", "evidence": "two"}
            ]}),
            "acceptance-report",
        ));
        let err = parsed.error.as_deref().expect("duplicate ids are an error");
        assert!(
            err.contains("criteriaSatisfied[1].id: duplicate normalized criterion id 'c-1'"),
            "{err}"
        );

        let unsupported = parse_acceptance_report(&report_text(
            json!({"criteriaSatisfied": [
                {"id": "c1", "status": "satisfied", "evidence": "one", "confidence": 0.9}
            ]}),
            "acceptance-report",
        ));
        let err = unsupported
            .error
            .as_deref()
            .expect("unknown fields are an error");
        assert!(
            err.contains("criteriaSatisfied[0].confidence: unsupported acceptance criterion field"),
            "{err}"
        );

        let stray = parse_acceptance_report(&report_text(
            json!({"totallyUnknown": 1}),
            "acceptance-report",
        ));
        let err = stray
            .error
            .as_deref()
            .expect("unknown report fields are an error");
        assert!(
            err.contains("totallyUnknown: unsupported acceptance report field"),
            "{err}"
        );
    }

    #[test]
    fn unwraps_every_wrapper_spelling_and_flags_siblings() {
        for wrapper in [
            "acceptance",
            "acceptance-report",
            "acceptance_report",
            "acceptanceReport",
        ] {
            let mut map = serde_json::Map::new();
            map.insert(wrapper.to_string(), report_value(json!({})));
            let body = Value::Object(map);
            let text = format!(
                "done\n```acceptance-report\n{}\n```",
                serde_json::to_string(&body).unwrap()
            );
            let parsed = parse_acceptance_report(&text);
            assert_eq!(parsed.error, None, "wrapper `{wrapper}` must unwrap");
            assert!(parsed.report.is_some(), "wrapper `{wrapper}` must unwrap");
        }

        let sibling = json!({"acceptance": report_value(json!({})), "extra": 1});
        let parsed = parse_acceptance_report(&format!(
            "done\n```acceptance-report\n{}\n```",
            serde_json::to_string(&sibling).unwrap()
        ));
        let err = parsed.error.as_deref().expect("a sibling key is an error");
        assert!(
            err.contains("extra: unsupported alongside acceptance report wrapper 'acceptance'"),
            "{err}"
        );

        let ambiguous = json!({
            "acceptance": report_value(json!({})),
            "acceptanceReport": report_value(json!({})),
        });
        let parsed = parse_acceptance_report(&format!(
            "done\n```acceptance-report\n{}\n```",
            serde_json::to_string(&ambiguous).unwrap()
        ));
        let err = parsed.error.as_deref().expect("two wrappers are ambiguous");
        assert!(
            err.contains("multiple acceptance report wrappers are ambiguous"),
            "{err}"
        );
    }

    #[test]
    fn recovers_an_unterminated_acceptance_report_fence() {
        let body = serde_json::to_string(&report_value(json!({}))).unwrap();
        let recovered = parse_acceptance_report(&format!("done\n```acceptance-report\n{body}"));
        assert_eq!(recovered.error, None);
        assert!(recovered.report.is_some());

        // The underscore spelling is accepted everywhere the hyphenated one is
        // (`acceptance.ts:702-703`).
        let underscored =
            parse_acceptance_report(&format!("done\n```acceptance_report\n{body}\n```"));
        assert_eq!(underscored.error, None);
        assert!(underscored.report.is_some());

        // An opened fence whose body is not JSON is reported as a defect of THIS report and is
        // never allowed to fall through to the generic-JSON or marker paths.
        let broken = parse_acceptance_report("done\n```acceptance-report\nnot json at all");
        assert!(broken.report.is_none());
        assert!(
            broken
                .error
                .as_deref()
                .unwrap()
                .starts_with("Failed to parse acceptance-report:"),
            "{:?}",
            broken.error
        );

        let empty = parse_acceptance_report("done\n```acceptance-report\n\n```");
        assert_eq!(
            empty.error.as_deref(),
            Some(
                "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
            )
        );
    }

    /// A model cut off mid-opener — `"…\n```acceptance-report"` with no newline after the tag —
    /// must still be reported as a FENCE DEFECT, not as "no report at all".
    ///
    /// `acceptance.ts:702`'s guard is `/```acceptance[-_]report\b/i.test(output)`: tag presence,
    /// with no `[^\n]*\n` anchor. Reusing the offset-finding opener helper (which needs the
    /// newline, per `acceptance.ts:671`) collapsed this case onto
    /// [`ACCEPTANCE_REPORT_NOT_FOUND`] — and that constant is the single discriminator both
    /// [`parse_acceptance_report_sources`] and [`crate::exec::acceptance::select_acceptance_report_source`] use to
    /// decide a report is genuinely ABSENT and the other source may be consulted. A truncated
    /// `file-only` artifact would therefore have been silently replaced by the assistant text.
    #[test]
    fn a_fence_opener_with_no_trailing_newline_is_a_defect_not_an_absent_report() {
        for opener in [
            "done\n```acceptance-report",
            "done\n```acceptance_report",
            // Trailing info-string content, still with no newline (`\b` then `[^\n]*`).
            "done\n```acceptance-report ",
        ] {
            let parsed = parse_acceptance_report(opener);
            assert!(parsed.report.is_none(), "{opener:?} -> {:?}", parsed.report);
            assert_eq!(
                parsed.error.as_deref(),
                Some(
                    "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
                ),
                "{opener:?} must report the fence defect, never {ACCEPTANCE_REPORT_NOT_FOUND:?}"
            );
        }

        // `\b` still bites: a tag that runs straight into another word character is NOT an
        // acceptance-report fence, so this one genuinely IS absent.
        let unrelated = parse_acceptance_report("done\n```acceptance-reporting");
        assert_eq!(
            unrelated.error.as_deref(),
            Some(ACCEPTANCE_REPORT_NOT_FOUND)
        );

        // And the load-bearing consequence: an absent report falls through to the other
        // source, but this defect must NOT — it is surfaced verbatim.
        let file = crate::exec::acceptance::AcceptanceFileOutput {
            content: "",
            path: std::path::Path::new("out.md"),
            authoritative: false,
        };
        let sources = parse_acceptance_report_sources("done\n```acceptance-report", Some(&file));
        assert!(sources.report.is_none());
        assert_eq!(
            sources.error.as_deref(),
            Some(
                "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
            )
        );
    }

    #[test]
    fn report_shaped_generic_json_surfaces_its_validation_errors() {
        // v0.34.0 swallowed this: a `json` fence that is unmistakably a report but has one bad
        // field read as unrelated prose, so the run silently had "no report".
        let text = format!(
            "prose\n```json\n{}\n```",
            serde_json::to_string(&json!({
                "criteriaSatisfied": [{"id": "c1", "status": "maybe", "evidence": "x"}],
                "changedFiles": ["a.rs"],
            }))
            .unwrap()
        );
        let parsed = parse_acceptance_report(&text);
        assert!(parsed.report.is_none());
        let err = parsed
            .error
            .as_deref()
            .expect("a report-shaped json fence reports errors");
        assert!(
            err.starts_with("Failed to parse acceptance-report: Invalid acceptance-report:"),
            "{err}"
        );

        // Genuinely unrelated JSON stays quiet.
        let unrelated = parse_acceptance_report("prose\n```json\n{\"hello\": \"world\"}\n```");
        assert_eq!(
            unrelated.error.as_deref(),
            Some(ACCEPTANCE_REPORT_NOT_FOUND)
        );
    }

    #[test]
    fn marker_path_distinguishes_missing_from_unterminated_objects() {
        assert_eq!(
            parse_acceptance_report("ACCEPTANCE_REPORT: nope")
                .error
                .as_deref(),
            Some(
                "Failed to parse acceptance-report: Expected a JSON object after ACCEPTANCE_REPORT:."
            )
        );
        assert_eq!(
            parse_acceptance_report("ACCEPTANCE_REPORT: {\"changedFiles\": [")
                .error
                .as_deref(),
            Some(
                "Failed to parse acceptance-report: Unterminated JSON object after ACCEPTANCE_REPORT:."
            )
        );
    }

    #[test]
    fn report_sources_prefer_the_file_when_authoritative_and_never_paper_over_a_defect() {
        let good = format!(
            "done\n```acceptance-report\n{}\n```",
            serde_json::to_string(&report_value(json!({"diffSummary": "from-file"}))).unwrap()
        );
        let path = std::path::Path::new("out.md");

        // Not authoritative: the assistant output is primary, the file is the fallback.
        let from_file_fallback = parse_acceptance_report_sources(
            "no report here",
            Some(&crate::exec::acceptance::AcceptanceFileOutput {
                content: &good,
                path,
                authoritative: false,
            }),
        );
        assert_eq!(
            from_file_fallback
                .report
                .as_ref()
                .and_then(|r| r.diff_summary.as_deref()),
            Some("from-file")
        );

        // Authoritative (`outputMode: "file-only"`): the file is searched FIRST and wins even
        // when the assistant output also carries a report.
        let assistant = format!(
            "done\n```acceptance-report\n{}\n```",
            serde_json::to_string(&report_value(json!({"diffSummary": "from-text"}))).unwrap()
        );
        let file_first = parse_acceptance_report_sources(
            &assistant,
            Some(&crate::exec::acceptance::AcceptanceFileOutput {
                content: &good,
                path,
                authoritative: true,
            }),
        );
        assert_eq!(
            file_first
                .report
                .as_ref()
                .and_then(|r| r.diff_summary.as_deref()),
            Some("from-file")
        );

        // A MALFORMED report in the primary source is surfaced, never papered over with the
        // secondary (`acceptance.ts:767-771`) — and the file's parse errors carry the path.
        let malformed =
            "done\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
        let primary_defect = parse_acceptance_report_sources(
            malformed,
            Some(&crate::exec::acceptance::AcceptanceFileOutput {
                content: &good,
                path,
                authoritative: false,
            }),
        );
        assert!(primary_defect.report.is_none());
        assert!(
            primary_defect
                .error
                .as_deref()
                .unwrap()
                .contains("Invalid acceptance-report"),
            "{:?}",
            primary_defect.error
        );

        let file_defect = parse_acceptance_report_sources(
            "no report here",
            Some(&crate::exec::acceptance::AcceptanceFileOutput {
                content: malformed,
                path,
                authoritative: false,
            }),
        );
        assert!(
            file_defect
                .error
                .as_deref()
                .unwrap()
                .ends_with("(in configured output out.md)"),
            "{:?}",
            file_defect.error
        );
    }

    // ---- G79: normalizedToken's second pass + the two `skip|skipped` alias groups ----

    /// G79 — `normalizedToken`'s SECOND `/-+/g` pass (`acceptance.ts:514`).
    ///
    /// The first pass (`/[\s_]+/g -> "-"`) only collapses WHITESPACE and UNDERSCORE runs; a
    /// literal `-` the model typed is left alone, so `"Not - Run"` comes out of pass one as
    /// `not---run` and `"NOT__-_SATISFIED"` as `not--satisfied`. Only the second pass folds
    /// those into the canonical single dash. Without it the alias lookup misses and the token
    /// falls through UNCHANGED, which surfaces as a report-validation error rejecting the whole
    /// run over the child's spacing.
    ///
    /// Driven through `parse_acceptance_report`, the live entry point `evaluate_acceptance`
    /// uses — not through the private helper.
    #[test]
    fn a_dashed_and_spaced_status_still_normalizes_onto_the_canonical_token() {
        let parsed = parse_acceptance_report(&report_text(
            json!({
                "criteriaSatisfied": [
                    {"id": "C -- 1", "status": "NOT__-_SATISFIED", "evidence": "not done"}
                ],
                "commandsRun": [
                    {"command": "cargo test", "result": "Not - Run", "summary": "skipped"}
                ]
            }),
            "acceptance-report",
        ));

        assert_eq!(
            parsed.error, None,
            "a second `-` collapse pass is what makes these tokens recognizable"
        );
        let report = parsed.report.expect("the normalized report parses");
        let criterion = &report.criteria_satisfied.as_ref().unwrap()[0];
        assert_eq!(
            criterion.id.as_deref(),
            Some("c-1"),
            "`C -- 1` -> `c---1` after pass one -> `c-1` after pass two"
        );
        assert_eq!(criterion.status, CriterionStatus::NotSatisfied);
        assert_eq!(
            report.commands_run.as_ref().unwrap()[0].result,
            CommandRunResult::NotRun,
            "`Not - Run` -> `not---run` after pass one -> `not-run` after pass two"
        );
    }

    /// G79 — the two alias groups the existing normalization test never touches:
    /// `normalizeCommandResult`'s `not-run|not-executed|skip|skipped` (`acceptance.ts:531`) and
    /// `normalizeCriterionStatus`'s `not-applicable|n-a|na|skip|skipped` (`:522`).
    ///
    /// `skip`/`skipped` appear in BOTH tables and mean different canonical values depending on
    /// which field they land in — the one place these tables are easy to get wrong.
    #[test]
    fn the_not_run_and_not_applicable_alias_groups_fold_onto_their_canonical_values() {
        for alias in ["not-run", "not_executed", "NOT EXECUTED", "skip", "Skipped"] {
            let parsed = parse_acceptance_report(&report_text(
                json!({"commandsRun": [
                    {"command": "cargo test", "result": alias, "summary": "not attempted"}
                ]}),
                "acceptance-report",
            ));
            assert_eq!(
                parsed.error, None,
                "`{alias}` must be a recognized commandsRun result"
            );
            let report = parsed.report.expect("the report parses");
            assert_eq!(
                report.commands_run.as_ref().unwrap()[0].result,
                CommandRunResult::NotRun,
                "`{alias}` is an alias of `not-run` (acceptance.ts:531)"
            );
        }

        // NB `n-a`, not `n/a`: `normalizedToken` collapses whitespace and `_`, never `/`, so a
        // literal slash is NOT an alias upstream either (`acceptance.ts:513-514,522`).
        for alias in ["not-applicable", "N A", "n_a", "na", "skip", "Skipped"] {
            let parsed = parse_acceptance_report(&report_text(
                json!({"criteriaSatisfied": [
                    {"id": "c1", "status": alias, "evidence": "does not apply here"}
                ]}),
                "acceptance-report",
            ));
            assert_eq!(
                parsed.error, None,
                "`{alias}` must be a recognized criterion status"
            );
            let report = parsed.report.expect("the report parses");
            assert_eq!(
                report.criteria_satisfied.as_ref().unwrap()[0].status,
                CriterionStatus::NotApplicable,
                "`{alias}` is an alias of `not-applicable` (acceptance.ts:522)"
            );
        }

        // A token that is in NEITHER table is still returned unchanged and then rejected by the
        // enum check, so the alias tables cannot be widened by accident.
        let unknown = parse_acceptance_report(&report_text(
            json!({"commandsRun": [
                {"command": "cargo test", "result": "deferred", "summary": "later"}
            ]}),
            "acceptance-report",
        ));
        assert!(
            unknown
                .error
                .as_deref()
                .unwrap()
                .contains("commandsRun[0].result: expected one of \"passed\", \"failed\", \"not-run\"; got \"deferred\""),
            "{unknown:?}"
        );
    }
}
