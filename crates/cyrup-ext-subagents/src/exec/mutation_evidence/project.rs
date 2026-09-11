//! pi `projectTimeoutRecovery` (`:114-135`) + `formatTimeoutRecoveryLines` (`:138-149`).
//!
//! Upstream's `formatTimeoutRecoveryLines(value: unknown, indent)` re-projects internally, which
//! lets it accept both a full summary (from a status step) and an already-narrowed projection
//! (from a wait child) — TypeScript erases the difference. Rust cannot, so the composition is
//! split explicitly: [`project_timeout_recovery`] is the ONE tolerant JSON reader,
//! [`format_timeout_recovery_lines`] renders a typed projection, and
//! [`format_timeout_recovery_lines_from_json`] chains the two for the status readers.

use super::types::{
    MAX_TIMEOUT_FILES, RecoveryReason, ReportStatus, Termination, TimeoutRecoveryProjection,
};

/// pi `projectTimeoutRecovery` (`:114-135`).
///
/// Takes `Option<&serde_json::Value>` rather than a typed struct because it projects from a
/// payload that may have been written by another build: an unknown `termination` or `reportStatus`
/// must degrade rather than fail a status read. This is the ONE tolerant reader in the module;
/// everything downstream of it is typed (a summary this build itself constructed narrows through
/// [`super::TimeoutRecoverySummary::project`] instead).
///
/// Two whole-object rules from `:117-118`, and one element-wise rule from `:119` — the contrast is
/// deliberate:
///
/// * an unrecognised `termination`, **or** a missing/non-array `changedFiles`, rejects the WHOLE
///   object (`:118`) — a missing `termination` is not a defaulted `"stopped"`;
/// * a non-string ELEMENT of `changedFiles` is filtered out without rejecting (`:119`).
///
/// `truncated` is set when the source says so **or** when the slice to [`MAX_TIMEOUT_FILES`]
/// actually dropped entries (`:130`). The second disjunct is how a projection stays honest about
/// its own truncation, and it is why the slice happens here even though the summary already
/// sliced: this may be reading a payload written by a build with a different cap.
///
/// `recoveryNeeded` and `reason` are read INDEPENDENTLY (`:131-132`) — foreign JSON may carry one
/// without the other, and the tolerant reader must remain tolerant (SCOPE_3 §A.5).
#[must_use]
pub fn project_timeout_recovery(
    value: Option<&serde_json::Value>,
) -> Option<TimeoutRecoveryProjection> {
    // `as_object` is `None` for null, scalars AND arrays — pi's `!value || typeof value !==
    // "object" || Array.isArray(value)` triple in one probe (`:117`).
    let source = value?.as_object()?;
    let termination = match source
        .get("termination")
        .and_then(serde_json::Value::as_str)
    {
        Some("timed-out") => Termination::TimedOut,
        Some("stopped") => Termination::Stopped,
        _ => return None, // whole-object reject (`:118`)
    };
    let all_changed_files: Vec<String> = source
        .get("changedFiles")?
        .as_array()? // whole-object reject for a missing or non-array field (`:118`)
        .iter()
        .filter_map(|element| element.as_str().map(str::to_string)) // element-wise filter (`:119`)
        .collect();
    let changed_files: Vec<String> = all_changed_files
        .iter()
        .take(MAX_TIMEOUT_FILES)
        .cloned()
        .collect();
    let report_status = match source
        .get("reportStatus")
        .and_then(serde_json::Value::as_str)
    {
        Some("missing") => Some(ReportStatus::Missing),
        Some("written") => Some(ReportStatus::Written),
        Some("not-requested") => Some(ReportStatus::NotRequested),
        Some("unknown") => Some(ReportStatus::Unknown),
        _ => None, // field-level degrade, never a reject (`:121-125`)
    };
    Some(TimeoutRecoveryProjection {
        termination,
        truncated: source.get("truncated").and_then(serde_json::Value::as_bool) == Some(true)
            || all_changed_files.len() > changed_files.len(), // `:130`
        changed_files,
        recovery_needed: source
            .get("recoveryNeeded")
            .and_then(serde_json::Value::as_bool)
            == Some(true), // `:131`
        reason: (source.get("reason").and_then(serde_json::Value::as_str)
            == Some("timed-out-with-dirty-worktree"))
        .then_some(RecoveryReason::TimedOutWithDirtyWorktree), // `:132`
        report_status,
    })
}

/// pi `formatTimeoutRecoveryLines` (`:138-149`), post-projection.
///
/// Returns **empty** unless `recovery_needed` is set (`:140`). That early return is what keeps an
/// ordinary timeout quiet, and SCOPE_3i's `recovery_note` depends on it.
///
/// Indent widths in use: `""` (SCOPE_3i / pi `subagent-wait.ts:357`), `"  "`
/// ([`crate::background::run_status`] / pi `run-status.ts:630`, `:754`, `:761`), `"    "`
/// (pi `async-status.ts:728`).
#[must_use]
pub fn format_timeout_recovery_lines(
    recovery: Option<&TimeoutRecoveryProjection>,
    indent: &str,
) -> Vec<String> {
    let Some(recovery) = recovery else {
        return Vec::new();
    };
    if !recovery.recovery_needed {
        return Vec::new(); // `:140`
    }
    // `:141-144` — note `", …"` is ONE U+2026 (the summary MESSAGE's marker is three ASCII dots;
    // see `format_path_list`), and the count carries a `"+"` suffix when truncated.
    let changed_files = if recovery.changed_files.is_empty() {
        "none".to_string()
    } else {
        format!(
            "{}{}",
            recovery.changed_files.join(", "),
            if recovery.truncated { ", …" } else { "" }
        )
    };
    let changed_file_count = if recovery.changed_files.is_empty() {
        "0".to_string()
    } else {
        format!(
            "{}{}",
            recovery.changed_files.len(),
            if recovery.truncated { "+" } else { "" }
        )
    };
    // `:147` — an ABSENT `reportStatus` renders as `"unknown"`, so `Unknown` and `None` display
    // identically by design; classification prefers `reason` and falls back to `termination`.
    let report = recovery
        .report_status
        .map_or_else(|| "unknown".to_string(), |status| status.to_string());
    let classification = recovery.reason.map_or_else(
        || recovery.termination.to_string(),
        |reason| reason.to_string(),
    );
    vec![
        format!(
            "{indent}Recovery needed: review the diff and artifacts before resuming or launching dependent stages."
        ),
        format!(
            "{indent}Recovery evidence: requested report: {report}; changed tracked files: {changed_files} ({changed_file_count}); classification: {classification}"
        ),
    ]
}

/// The raw-JSON entry point — [`project_timeout_recovery`] then
/// [`format_timeout_recovery_lines`]. This is what the status readers use, since a status step
/// carries the full SUMMARY (pi `shared/types.ts:1917`) and must be narrowed before rendering (pi
/// does the same narrowing inside its single function).
#[must_use]
pub fn format_timeout_recovery_lines_from_json(
    value: Option<&serde_json::Value>,
    indent: &str,
) -> Vec<String> {
    format_timeout_recovery_lines(project_timeout_recovery(value).as_ref(), indent)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn recovery_payload() -> serde_json::Value {
        serde_json::json!({
            "termination": "timed-out",
            "changedFiles": ["src/a.rs", "src/b.rs"],
            "recoveryNeeded": true,
            "reason": "timed-out-with-dirty-worktree",
            "reportStatus": "missing",
            // Summary-only fields the projection must NOT carry across:
            "message": "Recovery summary:\n…",
            "warning": "Inspect partial changes before retrying or resuming the child.",
            "sessionFile": "/tmp/session.jsonl",
        })
    }

    #[test]
    fn projects_the_narrow_subset_and_drops_every_path_and_message() {
        let projection = project_timeout_recovery(Some(&recovery_payload())).unwrap();
        assert_eq!(projection.termination, Termination::TimedOut);
        assert_eq!(projection.changed_files, vec!["src/a.rs", "src/b.rs"]);
        assert!(projection.recovery_needed);
        assert_eq!(
            projection.reason,
            Some(RecoveryReason::TimedOutWithDirtyWorktree)
        );
        assert_eq!(projection.report_status, Some(ReportStatus::Missing));
        let json = serde_json::to_value(&projection).unwrap();
        for leaked in [
            "message",
            "warning",
            "sessionFile",
            "transcriptPath",
            "artifactPaths",
        ] {
            assert!(
                json.get(leaked).is_none(),
                "{leaked} leaked into the projection"
            );
        }
    }

    #[test]
    fn rejects_the_whole_object_for_unknown_termination_or_non_array_changed_files() {
        let mut bad_termination = recovery_payload();
        bad_termination["termination"] = serde_json::json!("exploded");
        assert_eq!(project_timeout_recovery(Some(&bad_termination)), None);

        let mut missing_termination = recovery_payload();
        missing_termination
            .as_object_mut()
            .unwrap()
            .remove("termination");
        assert_eq!(project_timeout_recovery(Some(&missing_termination)), None);

        let mut non_array = recovery_payload();
        non_array["changedFiles"] = serde_json::json!("src/a.rs");
        assert_eq!(project_timeout_recovery(Some(&non_array)), None);

        assert_eq!(project_timeout_recovery(None), None);
        assert_eq!(
            project_timeout_recovery(Some(&serde_json::json!(null))),
            None
        );
        assert_eq!(
            project_timeout_recovery(Some(&serde_json::json!([1, 2]))),
            None
        );
        assert_eq!(
            project_timeout_recovery(Some(&serde_json::json!("text"))),
            None
        );
    }

    #[test]
    fn filters_non_string_elements_without_rejecting() {
        let mut mixed = recovery_payload();
        mixed["changedFiles"] = serde_json::json!(["src/a.rs", 7, null, "src/b.rs", {"k": 1}]);
        let projection = project_timeout_recovery(Some(&mixed)).unwrap();
        assert_eq!(projection.changed_files, vec!["src/a.rs", "src/b.rs"]);
        assert!(!projection.truncated, "filtering is not truncation");
    }

    #[test]
    fn degrades_unknown_field_values_without_rejecting() {
        let mut odd = recovery_payload();
        odd["reportStatus"] = serde_json::json!("half-written");
        odd["reason"] = serde_json::json!("some-other-reason");
        odd["recoveryNeeded"] = serde_json::json!("yes"); // non-boolean → not true
        let projection = project_timeout_recovery(Some(&odd)).unwrap();
        assert_eq!(projection.report_status, None);
        assert_eq!(projection.reason, None);
        assert!(!projection.recovery_needed);
    }

    #[test]
    fn tolerates_a_foreign_payload_where_the_pair_disagrees() {
        // §A.5: the tolerant reader must remain tolerant — `recoveryNeeded` without a `reason`
        // (and vice versa) both project rather than reject.
        let mut needed_only = recovery_payload();
        needed_only.as_object_mut().unwrap().remove("reason");
        let projection = project_timeout_recovery(Some(&needed_only)).unwrap();
        assert!(projection.recovery_needed);
        assert_eq!(projection.reason, None);

        let mut reason_only = recovery_payload();
        reason_only
            .as_object_mut()
            .unwrap()
            .remove("recoveryNeeded");
        let projection = project_timeout_recovery(Some(&reason_only)).unwrap();
        assert!(!projection.recovery_needed);
        assert_eq!(
            projection.reason,
            Some(RecoveryReason::TimedOutWithDirtyWorktree)
        );
    }

    #[test]
    fn sets_truncated_when_its_own_slice_drops_entries() {
        let files: Vec<String> = (0..23).map(|i| format!("f{i:02}.rs")).collect();
        let payload = serde_json::json!({
            "termination": "timed-out",
            "changedFiles": files,
        });
        let projection = project_timeout_recovery(Some(&payload)).unwrap();
        assert_eq!(projection.changed_files.len(), MAX_TIMEOUT_FILES);
        assert!(projection.truncated, "pi `:130`'s second disjunct");

        let honest = serde_json::json!({
            "termination": "stopped",
            "changedFiles": ["a.rs"],
            "truncated": true,
        });
        let projection = project_timeout_recovery(Some(&honest)).unwrap();
        assert!(projection.truncated, "pi `:130`'s first disjunct");
    }

    #[test]
    fn renderer_is_empty_without_recovery_needed_and_for_no_projection() {
        assert!(format_timeout_recovery_lines(None, "").is_empty());
        let quiet = serde_json::json!({
            "termination": "timed-out",
            "changedFiles": ["a.rs"],
        });
        // An ordinary timeout stays quiet (`:140`).
        assert!(format_timeout_recovery_lines_from_json(Some(&quiet), "").is_empty());
    }

    #[test]
    fn renderer_emits_the_two_upstream_lines_and_honours_every_indent_width() {
        let payload = recovery_payload();
        for indent in ["", "  ", "    "] {
            let lines = format_timeout_recovery_lines_from_json(Some(&payload), indent);
            assert_eq!(
                lines,
                vec![
                    format!(
                        "{indent}Recovery needed: review the diff and artifacts before resuming or launching dependent stages."
                    ),
                    format!(
                        "{indent}Recovery evidence: requested report: missing; changed tracked files: src/a.rs, src/b.rs (2); classification: timed-out-with-dirty-worktree"
                    ),
                ]
            );
        }
    }

    #[test]
    fn renderer_uses_the_single_ellipsis_and_the_plus_count_when_truncated() {
        let files: Vec<String> = (0..21).map(|i| format!("f{i:02}.rs")).collect();
        let payload = serde_json::json!({
            "termination": "timed-out",
            "changedFiles": files,
            "recoveryNeeded": true,
        });
        let lines = format_timeout_recovery_lines_from_json(Some(&payload), "");
        let evidence_line = lines.get(1).unwrap();
        // `:142` — ONE U+2026, not the summary message's three ASCII dots…
        assert!(evidence_line.contains(", …"), "{evidence_line}");
        assert!(!evidence_line.contains("..."), "{evidence_line}");
        // …and `:144` — the count carries `+`.
        assert!(evidence_line.contains("(20+)"), "{evidence_line}");
        // `reason` absent → classification falls back to termination (`:147`).
        assert!(
            evidence_line.ends_with("classification: timed-out"),
            "{evidence_line}"
        );
    }

    #[test]
    fn renderer_reports_unknown_for_an_absent_report_status_and_none_for_no_files() {
        let payload = serde_json::json!({
            "termination": "stopped",
            "changedFiles": [],
            "recoveryNeeded": true,
        });
        let lines = format_timeout_recovery_lines_from_json(Some(&payload), "");
        assert_eq!(
            lines.get(1).map(String::as_str),
            Some(
                "Recovery evidence: requested report: unknown; changed tracked files: none (0); classification: stopped"
            )
        );
    }
}
