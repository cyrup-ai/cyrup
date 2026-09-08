//! pi `buildTimeoutRecoverySummary` (`:151-199`) + `formatPathList` (`:107-111`).

use std::path::Path;

use super::types::{
    MAX_TIMEOUT_FILES, RecoveryNeeded, RecoveryReason, ReportStatus, Termination,
    TimeoutRecoverySummary, TrackedMutationEvidence,
};

/// The fixed warning (pi `:162`), stored on the summary as well as rendered into its `message`.
const WARNING: &str = "Inspect partial changes before retrying or resuming the child.";

/// pi `formatPathList` (`:107-111`): `"none"` for empty; otherwise the first
/// [`MAX_TIMEOUT_FILES`] joined by `", "`, and when more were dropped, `", ... (N more)"` —
/// **three ASCII dots plus a count**, deliberately DIFFERENT from the renderer's `", …"`
/// single-U+2026 marker ([`super::format_timeout_recovery_lines`]); both are operator-facing and
/// both are upstream's own characters.
fn format_path_list(paths: &[String]) -> String {
    if paths.is_empty() {
        return "none".to_string();
    }
    let shown = paths
        .iter()
        .take(MAX_TIMEOUT_FILES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > MAX_TIMEOUT_FILES {
        format!("{shown}, ... ({} more)", paths.len() - MAX_TIMEOUT_FILES)
    } else {
        shown
    }
}

/// The nine inputs of pi `buildTimeoutRecoverySummary` (`:151-160`) as a struct — a nine-argument
/// function is unreadable, and the compiler will not catch a transposed pair of `Option`s.
pub struct TimeoutRecoveryInput<'a> {
    pub termination: Termination,
    pub evidence: &'a TrackedMutationEvidence,
    /// pi `requiredOutputMissing?: boolean` (`:154`) — **`Option<bool>`, not `bool`**. The three
    /// states are distinct (`:164-166`): `Some(true)` ⇒ [`ReportStatus::Missing`], `Some(false)` ⇒
    /// [`ReportStatus::Written`], `None` ⇒ [`ReportStatus::NotRequested`]. Collapsing it to `bool`
    /// makes the `NotRequested` arm unreachable and silently promotes every no-output-contract run
    /// to a recovery-needed one.
    pub required_output_missing: Option<bool>,
    pub current_tool: Option<&'a str>,
    pub current_tool_args: Option<&'a str>,
    pub current_path: Option<&'a str>,
    pub session_file: Option<&'a Path>,
    pub transcript_path: Option<&'a Path>,
    pub artifact_paths: Option<&'a crate::artifacts::ArtifactPaths>,
}

/// pi `buildTimeoutRecoverySummary` (`:151-199`): fold the mutation evidence plus the run's live
/// context into one bounded, operator-facing [`TimeoutRecoverySummary`], `message` included.
#[must_use]
pub fn build_timeout_recovery_summary(input: TimeoutRecoveryInput<'_>) -> TimeoutRecoverySummary {
    let changed_files: Vec<String> = input
        .evidence
        .changed_files
        .iter()
        .take(MAX_TIMEOUT_FILES)
        .cloned()
        .collect();

    // pi `:164-166` — three-valued, and `Unknown` is never emitted here (it exists for foreign
    // payloads read back by the projection).
    let report_status = match input.required_output_missing {
        Some(true) => ReportStatus::Missing,
        Some(false) => ReportStatus::Written,
        None => ReportStatus::NotRequested,
    };

    // pi `:167-169`. A stopped child, a timed-out child that wrote its report, and a timed-out
    // child with a clean worktree all yield `false`. Widening any of the three makes every timeout
    // shout, which is worse than silence: `format_timeout_recovery_lines` returns EMPTY without
    // this flag (`:140`), so this boolean is the entire gate on whether a parent is interrupted at
    // all. Derived here, never stored independently: the `recoveryNeeded`/`reason` pair is ONE
    // `Option<RecoveryNeeded>` field (upstream's single spread at `:188`), so a summary claiming
    // `{ termination: Stopped, recoveryNeeded: true }` is unrepresentable from this constructor.
    let recovery_needed = matches!(input.termination, Termination::TimedOut)
        && matches!(report_status, ReportStatus::Missing)
        && !input.evidence.changed_files.is_empty();

    // Message assembly, in order — three unconditional lines (`:170-174`), then eight conditional
    // (`:175-182`), then the `Warning:` line (`:183`).
    let mut lines = vec![
        "Recovery summary:".to_string(),
        format!("- termination: {}", input.termination),
        format!(
            "- changed tracked files: {}",
            match &input.evidence.unavailable {
                Some(why) => format!("unavailable ({why})"),
                None => format_path_list(&input.evidence.changed_files),
            }
        ),
    ];
    if input.required_output_missing.is_some() {
        lines.push(format!("- requested report: {report_status}")); // :175
    }
    if recovery_needed {
        lines.push(
            "- Recovery needed: review the diff and artifacts before resuming or launching dependent stages."
                .to_string(),
        ); // :176
    }
    if let Some(tool) = input.current_tool {
        // :177 — an EM DASH, and only when the args preview is present.
        lines.push(match input.current_tool_args {
            Some(args) => format!("- active tool: {tool} — {args}"),
            None => format!("- active tool: {tool}"),
        });
    }
    if let Some(path) = input.current_path {
        lines.push(format!("- active path: {path}")); // :178
    }
    if let Some(path) = input.session_file {
        lines.push(format!("- session file: {}", path.display())); // :179
    }
    if let Some(path) = input.transcript_path {
        lines.push(format!("- transcript: {}", path.display())); // :180
    }
    if let Some(artifacts) = input.artifact_paths {
        lines.push(format!("- output artifact: {}", artifacts.output_path.display())); // :181
        lines.push(format!(
            "- metadata artifact: {}",
            artifacts.metadata_path.display()
        )); // :182
    }
    lines.push(format!("Warning: {WARNING}")); // :183

    TimeoutRecoverySummary {
        termination: input.termination,
        // pi `:187` — the slice above actually dropped entries OR the evidence was already
        // truncated.
        truncated: input.evidence.changed_files.len() > changed_files.len()
            || input.evidence.truncated,
        changed_files,
        recovery: recovery_needed.then_some(RecoveryNeeded {
            recovery_needed: true,
            reason: RecoveryReason::TimedOutWithDirtyWorktree,
        }),
        report_status: Some(report_status),
        current_tool: input.current_tool.map(str::to_string),
        current_tool_args: input.current_tool_args.map(str::to_string),
        current_path: input.current_path.map(str::to_string),
        session_file: input.session_file.map(Path::to_path_buf),
        transcript_path: input.transcript_path.map(Path::to_path_buf),
        artifact_paths: input.artifact_paths.cloned(),
        warning: WARNING.to_string(),
        message: lines.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::exec::mutation_evidence::types::TrackedMutationSource;

    fn evidence(changed: &[&str], truncated: bool) -> TrackedMutationEvidence {
        TrackedMutationEvidence {
            source: TrackedMutationSource::TrackedFiles,
            tracked_only: true,
            changed_files: changed.iter().map(ToString::to_string).collect(),
            attempted_mutation: !changed.is_empty(),
            truncated,
            unavailable: None,
        }
    }

    fn input<'a>(
        termination: Termination,
        evidence: &'a TrackedMutationEvidence,
        required_output_missing: Option<bool>,
    ) -> TimeoutRecoveryInput<'a> {
        TimeoutRecoveryInput {
            termination,
            evidence,
            required_output_missing,
            current_tool: None,
            current_tool_args: None,
            current_path: None,
            session_file: None,
            transcript_path: None,
            artifact_paths: None,
        }
    }

    #[test]
    fn recovery_needed_is_the_three_way_conjunction_and_nothing_else() {
        let dirty = evidence(&["a.rs"], false);
        let clean = evidence(&[], false);

        // All three conditions hold → the pair is present.
        let summary =
            build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, Some(true)));
        assert_eq!(
            summary.recovery,
            Some(RecoveryNeeded {
                recovery_needed: true,
                reason: RecoveryReason::TimedOutWithDirtyWorktree
            })
        );

        // A stopped child → false, even with a dirty worktree and a missing report.
        let summary =
            build_timeout_recovery_summary(input(Termination::Stopped, &dirty, Some(true)));
        assert_eq!(summary.recovery, None);

        // A timed-out child that WROTE its report → false.
        let summary =
            build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, Some(false)));
        assert_eq!(summary.recovery, None);

        // No output contract at all → false (`NotRequested`, not `Missing`).
        let summary = build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, None));
        assert_eq!(summary.recovery, None);
        assert_eq!(summary.report_status, Some(ReportStatus::NotRequested));

        // A timed-out child with a CLEAN worktree → false.
        let summary =
            build_timeout_recovery_summary(input(Termination::TimedOut, &clean, Some(true)));
        assert_eq!(summary.recovery, None);
    }

    #[test]
    fn message_carries_the_unconditional_prefix_and_the_warning() {
        let dirty = evidence(&["b.rs", "a.rs"], false);
        let summary = build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, None));
        let lines: Vec<&str> = summary.message.lines().collect();
        assert_eq!(lines.first(), Some(&"Recovery summary:"));
        assert_eq!(lines.get(1), Some(&"- termination: timed-out"));
        assert_eq!(lines.get(2), Some(&"- changed tracked files: b.rs, a.rs"));
        // `required_output_missing: None` → the `- requested report:` line is ABSENT (pi `:175`).
        assert!(!summary.message.contains("- requested report:"));
        assert_eq!(lines.last(), Some(&format!("Warning: {WARNING}").as_str()));
        assert_eq!(summary.warning, WARNING);
    }

    #[test]
    fn conditional_lines_render_in_upstream_order_with_the_em_dash() {
        let dirty = evidence(&["a.rs"], false);
        let artifacts = crate::artifacts::artifact_paths(
            Path::new("/artifacts"),
            "run-1",
            "coder",
            Some(2),
        );
        let mut built = input(Termination::TimedOut, &dirty, Some(true));
        built.current_tool = Some("edit");
        built.current_tool_args = Some("src/main.rs");
        built.current_path = Some("src/main.rs");
        built.session_file = Some(Path::new("/tmp/session.jsonl"));
        built.transcript_path = Some(Path::new("/artifacts/run-1_coder_2_transcript.jsonl"));
        built.artifact_paths = Some(&artifacts);
        let summary = build_timeout_recovery_summary(built);
        let expected = [
            "Recovery summary:",
            "- termination: timed-out",
            "- changed tracked files: a.rs",
            "- requested report: missing",
            "- Recovery needed: review the diff and artifacts before resuming or launching dependent stages.",
            "- active tool: edit — src/main.rs",
            "- active path: src/main.rs",
            "- session file: /tmp/session.jsonl",
            "- transcript: /artifacts/run-1_coder_2_transcript.jsonl",
            "- output artifact: /artifacts/run-1_coder_2_output.md",
            "- metadata artifact: /artifacts/run-1_coder_2_meta.json",
            &format!("Warning: {WARNING}"),
        ]
        .join("\n");
        assert_eq!(summary.message, expected);
    }

    #[test]
    fn active_tool_without_args_omits_the_em_dash_segment() {
        let dirty = evidence(&["a.rs"], false);
        let mut built = input(Termination::TimedOut, &dirty, None);
        built.current_tool = Some("bash");
        let summary = build_timeout_recovery_summary(built);
        assert!(summary.message.contains("- active tool: bash\n"));
        assert!(!summary.message.contains("—"));
    }

    #[test]
    fn unavailable_evidence_renders_the_reason_inline() {
        let unavailable = TrackedMutationEvidence {
            source: TrackedMutationSource::TrackedFiles,
            tracked_only: true,
            changed_files: Vec::new(),
            attempted_mutation: false,
            truncated: false,
            unavailable: Some("git repository discovery failed: not a repo".to_string()),
        };
        let summary =
            build_timeout_recovery_summary(input(Termination::TimedOut, &unavailable, None));
        assert!(summary.message.contains(
            "- changed tracked files: unavailable (git repository discovery failed: not a repo)"
        ));
    }

    #[test]
    fn the_summary_slices_to_the_wire_cap_and_the_message_uses_ascii_dots() {
        let files: Vec<String> = (0..25).map(|i| format!("f{i:02}.rs")).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let dirty = evidence(&refs, false);
        let summary = build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, None));
        assert_eq!(summary.changed_files.len(), MAX_TIMEOUT_FILES);
        assert!(summary.truncated, "its own slice dropped entries (pi `:187`)");
        // The summary MESSAGE's marker: three ASCII dots plus a count (pi `:110`) — deliberately
        // NOT the renderer's single-U+2026 `", …"`.
        assert!(summary.message.contains(", ... (5 more)"));
        assert!(!summary.message.contains('…'));
    }

    #[test]
    fn already_truncated_evidence_keeps_the_flag_even_under_the_cap() {
        let dirty = evidence(&["a.rs"], true);
        let summary = build_timeout_recovery_summary(input(Termination::TimedOut, &dirty, None));
        assert!(summary.truncated, "pi `:187`'s second disjunct");
    }

    #[test]
    fn empty_changed_files_render_as_none() {
        let clean = evidence(&[], false);
        let summary = build_timeout_recovery_summary(input(Termination::TimedOut, &clean, None));
        assert!(summary.message.contains("- changed tracked files: none"));
    }
}
