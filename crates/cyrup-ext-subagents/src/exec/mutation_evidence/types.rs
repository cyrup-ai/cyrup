//! The wire types of pi `runs/shared/mutation-evidence.ts` — `shared/types.ts:584-625`.
//!
//! Every optional-on-the-wire field is `#[serde(default, skip_serializing_if = …)]`: each of these
//! shapes round-trips through a `status.json` / result file that an older build may have written,
//! so absence must decode and `false`/`None` must not be written.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// pi `:8`. Caps the snapshot's path list AND is what makes a truncated snapshot refuse to call a
/// newly-appearing file "changed" (see
/// [`collect_tracked_mutation_evidence`](super::collect_tracked_mutation_evidence)).
pub(super) const MAX_TRACKED_PATHS: usize = 500;

/// pi `:9`. Upstream uses this as `execFileSync`'s `maxBuffer`; cyrup has no subprocess, so it
/// bounds the bytes hashed per file instead — a file larger than this is fingerprinted over its
/// first `MAX_HASH_BYTES` plus its length, which is what upstream's spill path effectively did.
pub(super) const MAX_HASH_BYTES: usize = 1024 * 1024;

/// pi `:10`. The wire/render cap, applied in BOTH
/// [`build_timeout_recovery_summary`](super::build_timeout_recovery_summary) (`:163`) and
/// [`project_timeout_recovery`](super::project_timeout_recovery) (`:120`) — the second is not
/// redundant: a projection may be reading a payload written by a build with a different cap.
pub(super) const MAX_TIMEOUT_FILES: usize = 20;

/// pi's literal-typed `source: "tracked-files"` discriminant (`shared/types.ts:585`). One variant:
/// the record itself says what kind of evidence it is, and there is only one kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackedMutationSource {
    #[default]
    TrackedFiles,
}

/// pi `TrackedMutationSnapshot` (`shared/types.ts:584-593`): the tracked-file baseline taken
/// before the first child spawns, against which
/// [`collect_tracked_mutation_evidence`](super::collect_tracked_mutation_evidence) measures what
/// the child changed.
///
/// `source` and `tracked_only` are pi's literal-typed discriminants (`"tracked-files"` / `true`).
/// They are not configurable and are serialized because the wire shape carries them: an untracked
/// build artefact is not evidence of a mutation the parent needs to review, and the record says so
/// about itself.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedMutationSnapshot {
    pub source: TrackedMutationSource,
    /// Always `true` — pi's literal `trackedOnly: true`.
    pub tracked_only: bool,
    /// The directory the snapshot was taken from (repository discovery starts here).
    pub cwd: PathBuf,
    /// [CYRUP-DELTA, fills an upstream hole] `shared/types.ts:588` declares `gitRoot` and
    /// `mutation-evidence.ts` never writes it. `gix::discover` yields the work-dir for free, so
    /// cyrup populates it; upstream would have needed a second `git` invocation. It costs nothing
    /// and makes an `unavailable` snapshot diagnosable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<PathBuf>,
    /// The repo-relative tracked paths already dirty at snapshot time, capped at
    /// [`MAX_TRACKED_PATHS`] (pi `:50`).
    pub dirty_files: Vec<String>,
    /// Per-path fingerprints of the dirty files above.
    ///
    /// `BTreeMap`, not `HashMap`: the snapshot is serialized and compared, and a stable key order
    /// keeps those diffable — the same reason `changed_files` is sorted at pi `:94`.
    pub fingerprints: BTreeMap<String, TrackedMutationFingerprint>,
    /// The dirty-path list hit [`MAX_TRACKED_PATHS`] and was cut (pi `truncated?: boolean`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Why the snapshot could not be taken (a non-repository cwd is the ordinary case, not an
    /// error). Present ⇒ `dirty_files`/`fingerprints` are empty and every later collect
    /// short-circuits (pi `:79-81`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// pi's `kind: "diff"` discriminant (`shared/types.ts:595`) — preserved for wire compatibility
/// even though cyrup's fingerprint algorithm differs (see [`super::repo`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FingerprintKind {
    #[default]
    Diff,
}

/// pi `TrackedMutationFingerprint` (`shared/types.ts:595`): a per-file "has this file's content
/// diverged from HEAD, and how" digest. Only ever compared against another fingerprint taken by
/// the same build within one run's lifetime — see [`super::repo`] for the algorithm and its
/// deliberate divergence from upstream's patch-text hash.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedMutationFingerprint {
    pub kind: FingerprintKind,
    pub digest: String,
}

/// pi `TrackedMutationEvidence` (`shared/types.ts:597-604`): the settled diff of two snapshots —
/// which tracked files the child changed between launch and settle.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedMutationEvidence {
    pub source: TrackedMutationSource,
    /// Always `true` — pi's literal `trackedOnly: true`.
    pub tracked_only: bool,
    /// Sorted (pi `:94` — rendered and compared; must be stable).
    pub changed_files: Vec<String>,
    /// `!changed_files.is_empty()` (pi `:99`).
    pub attempted_mutation: bool,
    /// Either endpoint's listing hit [`MAX_TRACKED_PATHS`] (pi `:100`'s disjunct).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Why evidence could not be collected — carried forward from the snapshot's own
    /// `unavailable`, or a collect-time failure. Present ⇒ `changed_files` is empty and
    /// `attempted_mutation` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// pi's four-valued `reportStatus` (`shared/types.ts:612`).
///
/// [`build_timeout_recovery_summary`](super::build_timeout_recovery_summary) emits only the first
/// three (`:164-166`); [`project_timeout_recovery`](super::project_timeout_recovery) accepts all
/// four (`:121-125`) because it reads payloads it did not write, and
/// [`format_timeout_recovery_lines`](super::format_timeout_recovery_lines) renders an ABSENT value
/// as `"unknown"` too (`:147`), so `Unknown` and `None` display identically by design.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportStatus {
    Missing,
    Written,
    NotRequested,
    Unknown,
}

impl std::fmt::Display for ReportStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "missing",
            Self::Written => "written",
            Self::NotRequested => "not-requested",
            Self::Unknown => "unknown",
        })
    }
}

/// pi's `termination` discriminant (`shared/types.ts:607`). `kebab-case` gives `"timed-out"` /
/// `"stopped"` verbatim.
///
/// Both of upstream's production build sites pass `"timed-out"` (`execution.ts:1505`,
/// `subagent-runner.ts:1485` — the latter's second disjunct, `ctx.timeoutSignal?.aborted`, is the
/// run-level DEADLINE signal, not a stop); `Stopped` is carried for wire compatibility, because
/// the type declares it and the tolerant reader must accept a foreign payload that says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Termination {
    TimedOut,
    Stopped,
}

impl std::fmt::Display for Termination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TimedOut => "timed-out",
            Self::Stopped => "stopped",
        })
    }
}

/// pi's `reason: "timed-out-with-dirty-worktree"` literal (`shared/types.ts:611`) — the one
/// recovery classification upstream ever writes (`:188`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryReason {
    TimedOutWithDirtyWorktree,
}

impl std::fmt::Display for RecoveryReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TimedOutWithDirtyWorktree => "timed-out-with-dirty-worktree",
        })
    }
}

/// The `recoveryNeeded`/`reason` pair — upstream `:188` emits them as ONE spread
/// (`...(recoveryNeeded ? { recoveryNeeded: true, reason: "timed-out-with-dirty-worktree" } : {})`),
/// so both keys appear together or neither.
///
/// Carrying the reason INSIDE the [`TimeoutRecoverySummary::recovery`] `Option` is what makes
/// `recoveryNeeded: true` without a reason — and a reason without `recoveryNeeded` — both
/// unrepresentable in the TYPED shape. `{ termination: Stopped, recoveryNeeded: true }` does not
/// compile out of [`build_timeout_recovery_summary`](super::build_timeout_recovery_summary): the
/// constructor computes the pair from the three-way conjunction (pi `:167-169`) and there is no
/// setter. [`project_timeout_recovery`](super::project_timeout_recovery) still reads the two wire
/// keys independently (`:131-132`), and still tolerates their disagreement, because it parses
/// foreign JSON — the SCOPE_3 §A.5 caveat: the tolerant reader must remain tolerant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryNeeded {
    /// Always `true` when the pair is present — upstream writes the literal `recoveryNeeded: true`.
    pub recovery_needed: bool,
    pub reason: RecoveryReason,
}

/// pi `TimeoutRecoverySummary` (`shared/types.ts:606-623`), built by
/// [`build_timeout_recovery_summary`](super::build_timeout_recovery_summary) (`:151-199`) for a
/// child killed by its deadline: bounded routing evidence — never raw output — plus the
/// operator-facing `message` the delivered output splices in.
///
/// The parent-facing subset that may cross into a tool result is [`TimeoutRecoveryProjection`],
/// NOT this: the full summary carries session, transcript and artifact paths that are local state
/// (a status step keeps them, `shared/types.ts:1917`; a job/wait view must narrow them away,
/// `shared/types.ts:1962-1965`'s `Omit &`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeoutRecoverySummary {
    pub termination: Termination,
    /// Capped at [`MAX_TIMEOUT_FILES`] (pi `:163`).
    pub changed_files: Vec<String>,
    /// The cap above actually dropped entries **or** the evidence was already truncated (pi
    /// `:187`'s disjunct).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The `recoveryNeeded`/`reason` pair, present iff the three-way conjunction held (pi
    /// `:167-169`, spread at `:188`) — see [`RecoveryNeeded`] for why this is one field.
    ///
    /// Wire shape proven by round-trip test: `Some` flattens to
    /// `"recoveryNeeded":true,"reason":"timed-out-with-dirty-worktree"`, `None` writes neither
    /// key, and a legacy payload with neither key decodes as `None`.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryNeeded>,
    /// `Option` because the wire declares it optional (`reportStatus?:`) and a foreign payload may
    /// omit it; [`build_timeout_recovery_summary`](super::build_timeout_recovery_summary) always
    /// stamps one of the first three values and never [`ReportStatus::Unknown`] (pi `:164-166`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_status: Option<ReportStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_paths: Option<crate::artifacts::ArtifactPaths>,
    /// The fixed warning (pi `:162`), stored on the summary as well as rendered into `message`.
    pub warning: String,
    /// The assembled operator-facing text (pi `:170-183` joined) — what
    /// [`crate::exec::run_sync`]'s timeout preamble splices into the delivered output.
    pub message: String,
}

impl TimeoutRecoverySummary {
    /// Narrow this summary to its parent-facing [`TimeoutRecoveryProjection`] — the typed
    /// counterpart of [`project_timeout_recovery`](super::project_timeout_recovery) for a summary
    /// this build itself constructed (upstream needs no separate function: TypeScript's
    /// `projectTimeoutRecovery` accepts both shapes because the difference is erased).
    ///
    /// Slices `changed_files` to [`MAX_TIMEOUT_FILES`] again and ORs its own drop into
    /// `truncated`, exactly as the JSON reader does (pi `:120`, `:130`): a projection stays honest
    /// about its own truncation even when the source was written under a different cap.
    #[must_use]
    pub fn project(&self) -> TimeoutRecoveryProjection {
        let changed_files: Vec<String> = self
            .changed_files
            .iter()
            .take(MAX_TIMEOUT_FILES)
            .cloned()
            .collect();
        TimeoutRecoveryProjection {
            termination: self.termination,
            truncated: self.truncated || self.changed_files.len() > changed_files.len(),
            changed_files,
            recovery_needed: self.recovery.is_some(),
            reason: self.recovery.map(|recovery| recovery.reason),
            report_status: self.report_status,
        }
    }
}

/// pi `TimeoutRecoveryProjection` (`shared/types.ts:625`) — the safe parent-facing subset:
/// `Pick<TimeoutRecoverySummary, "termination"|"changedFiles"|"truncated"|"recoveryNeeded"|"reason"|"reportStatus">`.
///
/// Declared as its own struct, not an alias: the narrowing is the point. This is what crosses
/// into a tool result (`WaitCompletionChild.timeoutRecovery`, `shared/types.ts:1350`; the
/// async-job view's `Omit & { timeoutRecovery?: TimeoutRecoveryProjection }`,
/// `shared/types.ts:1962-1965`), and the summary's `message`, `warning` and every path have no
/// business there.
///
/// `recovery_needed` and `reason` are INDEPENDENT here — unlike the summary's typed
/// [`RecoveryNeeded`] pair — because a projection is populated by the tolerant reader
/// ([`project_timeout_recovery`](super::project_timeout_recovery), pi `:131-132`), which must
/// accept a foreign payload where the two keys disagree.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeoutRecoveryProjection {
    pub termination: Termination,
    /// Capped at [`MAX_TIMEOUT_FILES`] (pi `:120`).
    pub changed_files: Vec<String>,
    /// Source said so **or** this projection's own slice dropped entries (pi `:130`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovery_needed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RecoveryReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_status: Option<ReportStatus>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn minimal_summary(termination: Termination, recovery: Option<RecoveryNeeded>) -> TimeoutRecoverySummary {
        TimeoutRecoverySummary {
            termination,
            changed_files: Vec::new(),
            truncated: false,
            recovery,
            report_status: None,
            current_tool: None,
            current_tool_args: None,
            current_path: None,
            session_file: None,
            transcript_path: None,
            artifact_paths: None,
            warning: "w".to_string(),
            message: "m".to_string(),
        }
    }

    /// The §A wire-shape proof from SCOPE_3c: `Some` flattens both keys, `None` writes neither,
    /// both directions round-trip, and a legacy payload with NEITHER key decodes as `None`.
    #[test]
    fn recovery_pair_flattens_to_upstreams_exact_wire_shape() {
        let some = minimal_summary(
            Termination::TimedOut,
            Some(RecoveryNeeded {
                recovery_needed: true,
                reason: RecoveryReason::TimedOutWithDirtyWorktree,
            }),
        );
        let json = serde_json::to_value(&some).unwrap();
        assert_eq!(json.get("recoveryNeeded"), Some(&serde_json::json!(true)));
        assert_eq!(
            json.get("reason"),
            Some(&serde_json::json!("timed-out-with-dirty-worktree"))
        );
        assert_eq!(json.get("termination"), Some(&serde_json::json!("timed-out")));
        let round: TimeoutRecoverySummary = serde_json::from_value(json).unwrap();
        assert_eq!(round, some);

        let none = minimal_summary(Termination::Stopped, None);
        let json = serde_json::to_value(&none).unwrap();
        assert!(json.get("recoveryNeeded").is_none());
        assert!(json.get("reason").is_none());
        let round: TimeoutRecoverySummary = serde_json::from_value(json).unwrap();
        assert_eq!(round, none);
    }

    #[test]
    fn legacy_payload_without_the_recovery_pair_decodes_as_none() {
        let legacy = serde_json::json!({
            "termination": "stopped",
            "changedFiles": ["a.rs"],
            "warning": "w",
            "message": "m",
        });
        let decoded: TimeoutRecoverySummary = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.recovery, None);
        assert_eq!(decoded.report_status, None);
        assert_eq!(decoded.changed_files, vec!["a.rs".to_string()]);
    }

    #[test]
    fn optional_wire_fields_are_omitted_not_written() {
        let json = serde_json::to_value(minimal_summary(Termination::TimedOut, None)).unwrap();
        for absent in [
            "truncated",
            "reportStatus",
            "currentTool",
            "currentToolArgs",
            "currentPath",
            "sessionFile",
            "transcriptPath",
            "artifactPaths",
        ] {
            assert!(json.get(absent).is_none(), "{absent} must be omitted");
        }
    }

    #[test]
    fn typed_projection_narrows_away_message_warning_and_every_path() {
        let mut summary = minimal_summary(
            Termination::TimedOut,
            Some(RecoveryNeeded {
                recovery_needed: true,
                reason: RecoveryReason::TimedOutWithDirtyWorktree,
            }),
        );
        summary.changed_files = (0..25).map(|i| format!("f{i:02}.rs")).collect();
        summary.report_status = Some(ReportStatus::Missing);
        summary.session_file = Some(PathBuf::from("/tmp/session.jsonl"));
        let projection = summary.project();
        assert_eq!(projection.changed_files.len(), MAX_TIMEOUT_FILES);
        assert!(projection.truncated, "its own slice dropped entries");
        assert!(projection.recovery_needed);
        assert_eq!(projection.reason, Some(RecoveryReason::TimedOutWithDirtyWorktree));
        assert_eq!(projection.report_status, Some(ReportStatus::Missing));
        let json = serde_json::to_value(&projection).unwrap();
        for leaked in ["message", "warning", "sessionFile", "transcriptPath", "artifactPaths"] {
            assert!(json.get(leaked).is_none(), "{leaked} must never cross into a projection");
        }
    }

    #[test]
    fn snapshot_and_evidence_round_trip_with_camel_case_keys() {
        let snapshot = TrackedMutationSnapshot {
            source: TrackedMutationSource::TrackedFiles,
            tracked_only: true,
            cwd: PathBuf::from("/repo"),
            git_root: Some(PathBuf::from("/repo")),
            dirty_files: vec!["src/a.rs".to_string()],
            fingerprints: [(
                "src/a.rs".to_string(),
                TrackedMutationFingerprint {
                    kind: FingerprintKind::Diff,
                    digest: "abc".to_string(),
                },
            )]
            .into_iter()
            .collect(),
            truncated: false,
            unavailable: None,
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json.get("source"), Some(&serde_json::json!("tracked-files")));
        assert_eq!(json.get("trackedOnly"), Some(&serde_json::json!(true)));
        assert_eq!(json.get("gitRoot"), Some(&serde_json::json!("/repo")));
        assert_eq!(
            json.pointer("/fingerprints/src~1a.rs/kind"),
            Some(&serde_json::json!("diff"))
        );
        let round: TrackedMutationSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(round, snapshot);
    }
}
