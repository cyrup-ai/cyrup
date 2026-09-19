//! Discovering handoff manifests, joining them to cleanup tasks, and deciding whether the owning
//! run has finished — pi `worktree-cleanup-plan.ts:303-441` and `:480-519` @`v0.68.0`.
//!
//! Nothing manifest-shaped is declared here. The records come from [`crate::handoff::model`],
//! which is the crate's ONE serialization of that file; a second one would be a coin flip on
//! which reader a writer's bytes reached.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::model::{MAX_DISCOVERED_HANDOFFS, MAX_METADATA_FILE_BYTES, MAX_PLAN_ENTRIES};
use super::paths::{comparable_path, metadata_relative_path};
use super::verdict::{ActiveReason, StaleReason, UnknownReason};
use crate::background::RunStatus;
use crate::handoff::model::{Child, Group, Manifest, WorktreeCleanupTask};

/// One joined `(manifest, group, cleanup task, child)` row — pi `ManifestMetadataRecord`
/// (`:104-112`).
///
/// The join is 1:1 by construction: [`manifest_metadata`] drops any task whose `taskIndex` does
/// not match EXACTLY one child, because a cleanup decision made against an ambiguous child row is
/// a decision made against evidence that does not identify what it describes.
#[derive(Clone, Debug)]
pub(crate) struct MetadataRecord {
    /// The manifest this row came from.
    pub(crate) manifest_path: PathBuf,
    /// The whole manifest — the classifier needs `runId`/`source` from it.
    pub(crate) manifest: Manifest,
    /// The group holding the task.
    pub(crate) group: Group,
    /// The cleanup task naming the worktree.
    pub(crate) task: WorktreeCleanupTask,
    /// The one child whose `taskIndex` matches.
    pub(crate) child: Child,
    /// `<dirname(manifest)>/status.json`, whether or not it exists.
    pub(crate) status_path: PathBuf,
    /// The owning run's status, when it parsed.
    pub(crate) status: Option<RunStatus>,
    /// The raw `state` string, when the status file exists but did not parse as a
    /// [`RunStatus`]. See [`inspect_run_state`]'s `[CYRUP-DELTA]`.
    pub(crate) status_raw_state: Option<String>,
    /// Why the status could not be read, when it could not.
    pub(crate) status_error: Option<String>,
}

impl MetadataRecord {
    /// pi `metadataRecordPath` (`:460-462`) — the worktree this row claims.
    pub(crate) fn worktree_path(&self) -> PathBuf {
        metadata_relative_path(&self.manifest_path, &self.task.path)
    }

    /// pi `metadataPatchPath` (`:464-467`).
    pub(crate) fn patch_path(&self) -> Option<PathBuf> {
        let path = &self.child.patch.path;
        (!path.as_os_str().is_empty()).then(|| metadata_relative_path(&self.manifest_path, path))
    }

    /// pi `metadataReportPaths` (`:475-479`) — output, structured output, transcript.
    pub(crate) fn report_paths(&self) -> Vec<PathBuf> {
        [
            self.child.output_path.as_ref(),
            self.child.structured_output_path.as_ref(),
            self.child.session_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| metadata_relative_path(&self.manifest_path, path))
        .collect()
    }

    /// pi `metadataOutputPaths` (`:469-473`) — the report paths PLUS the captured patch.
    pub(crate) fn output_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.report_paths();
        paths.extend(self.patch_path());
        paths
    }
}

/// Everything [`load_metadata`] found, plus the diagnostics it collected on the way.
pub(crate) struct LoadedMetadata {
    /// The joined rows.
    pub(crate) records: Vec<MetadataRecord>,
    /// Every manifest path consulted, sorted by comparable path.
    pub(crate) paths: Vec<PathBuf>,
    /// Deduped and sorted (pi `:440`).
    pub(crate) warnings: BTreeSet<String>,
}

/// pi `discoverHandoffPaths` (`:328-365`).
///
/// Upstream's own comment at `:333-335` is the rule that matters and it is ported with it: the
/// project artifacts directory has a fixed, shallow handoff layout, and this must NOT walk a temp
/// or session root, because those roots hold unrelated operator data and are not a source of
/// ownership for cleanup candidates.
fn discover_handoff_paths(
    repo_root: &Path,
    explicit: Option<&Path>,
    extra: &[PathBuf],
    warnings: &mut BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let add = |candidate: &Path, paths: &mut Vec<PathBuf>| {
        let resolved = if candidate.is_absolute() {
            std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf())
        } else {
            crate::spawn::worktree::lexical_normalize(repo_root, candidate)
        };
        let key = comparable_path(&resolved);
        if !paths
            .iter()
            .any(|existing| comparable_path(existing) == key)
        {
            paths.push(resolved);
        }
    };

    if let Some(explicit) = explicit {
        add(explicit, &mut paths);
    }
    for candidate in extra {
        add(candidate, &mut paths);
    }

    let artifacts_dir = crate::artifacts::project_artifacts_dir(repo_root);
    let project_handoff = artifacts_dir.join("handoff.json");
    if project_handoff.exists() {
        add(&project_handoff, &mut paths);
    }

    let handoffs_dir = artifacts_dir.join("handoffs");
    match std::fs::symlink_metadata(&handoffs_dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            warnings.insert(format!(
                "ignored non-directory handoff metadata root: {}",
                handoffs_dir.display()
            ));
        }
        Ok(_) => match std::fs::read_dir(&handoffs_dir) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.ends_with(".json"))
                    .collect();
                names.sort();
                let capped = names.len() > MAX_DISCOVERED_HANDOFFS;
                names.truncate(MAX_DISCOVERED_HANDOFFS);
                for name in names {
                    add(&handoffs_dir.join(name), &mut paths);
                }
                if capped {
                    warnings.insert(format!(
                        "handoff metadata discovery capped at {MAX_DISCOVERED_HANDOFFS} files"
                    ));
                }
            }
            Err(err) => {
                warnings.insert(format!(
                    "failed to inspect handoff metadata root {}: {err}",
                    handoffs_dir.display()
                ));
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            warnings.insert(format!(
                "failed to inspect handoff metadata root {}: {err}",
                handoffs_dir.display()
            ));
        }
    }

    paths.sort_by_key(|path| comparable_path(path));
    paths
}

/// pi `readManifest` (`:315-326`) — the bounded, symlink-refusing, size-capped read.
///
/// [CYRUP-DELTA] where pi hand-checks `version === 1 && Array.isArray(groups)` and then treats
/// every deeper defect as a per-group/per-task warning (`:387-399`), cyrup parses the file with
/// [`crate::handoff::read_manifest`] — the crate's ONE manifest reader, which validates identity
/// as well as shape. The divergence is deliberate on both counts. First, reuse: a second, laxer
/// parser for the same file is exactly the drift the sibling module's `deny_unknown_fields`
/// exists to prevent. Second, the failure mode is IDENTICAL in safety terms — pi's per-record
/// warnings and this reader's whole-file rejection both end with the affected worktrees producing
/// no `Safe` entry, and a worktree with no entry is a worktree nothing proposes removing. What
/// differs is granularity: one defective group here costs the analysis of its siblings in the
/// same file, and the warning says so.
async fn read_manifest_bounded(manifest_path: &Path) -> Result<Manifest, String> {
    let meta = match std::fs::symlink_metadata(manifest_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "parallel handoff manifest not found: {}",
                manifest_path.display()
            ));
        }
        Err(err) => {
            return Err(format!(
                "failed to read parallel handoff manifest {}: {err}",
                manifest_path.display()
            ));
        }
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(format!(
            "parallel handoff manifest is not a regular file: {}",
            manifest_path.display()
        ));
    }
    if meta.len() > MAX_METADATA_FILE_BYTES {
        return Err(format!(
            "parallel handoff manifest exceeds the {MAX_METADATA_FILE_BYTES}-byte limit: {}",
            manifest_path.display()
        ));
    }
    match crate::handoff::read_manifest(manifest_path).await {
        Ok(Some(manifest)) => Ok(manifest),
        Ok(None) => Err(format!(
            "parallel handoff manifest not found: {}",
            manifest_path.display()
        )),
        Err(err) => Err(format!(
            "invalid parallel handoff manifest {}: {err}",
            manifest_path.display()
        )),
    }
}

/// pi `readStatusBesideManifest` (`:368-385`).
///
/// [CYRUP-DELTA] cyrup's [`crate::background::RunState`] is a CLOSED enum (`queued | running |
/// paused | complete | failed | stopped`) where pi's `AsyncStatus["state"]` is an open string
/// union that additionally admits `partial` and `rejected`. A straight `serde` read would
/// therefore turn pi's *"owning run has unknown state 'X'"* into *"invalid async status"* — same
/// verdict (`Unknown`), different and much less useful sentence. So a parse failure falls back to
/// the raw JSON object and, when it carries a string `state` and a string `runId`, keeps pi's own
/// diagnostic. The closed enum is still the right Rust shape: it is what makes
/// [`terminal_run_state`] exhaustive.
fn read_status_beside_manifest(manifest_path: &Path) -> (PathBuf, StatusRead) {
    let status_path = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("status.json");
    let meta = match std::fs::symlink_metadata(&status_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (status_path, StatusRead::Absent);
        }
        Err(err) => {
            let message = format!(
                "failed to read async status {}: {err}",
                status_path.display()
            );
            return (status_path, StatusRead::Error(message));
        }
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        let message = format!(
            "async status is not a regular file: {}",
            status_path.display()
        );
        return (status_path, StatusRead::Error(message));
    }
    if meta.len() > MAX_METADATA_FILE_BYTES {
        let message = format!(
            "async status exceeds the {MAX_METADATA_FILE_BYTES}-byte limit: {}",
            status_path.display()
        );
        return (status_path, StatusRead::Error(message));
    }
    let bytes = match std::fs::read(&status_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let message = format!(
                "failed to read async status {}: {err}",
                status_path.display()
            );
            return (status_path, StatusRead::Error(message));
        }
    };
    if let Ok(status) = serde_json::from_slice::<RunStatus>(&bytes) {
        return (status_path, StatusRead::Parsed(Box::new(status)));
    }
    // The fallback described above: keep pi's sentence for a state this build does not model.
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value)
            if value
                .get("runId")
                .and_then(serde_json::Value::as_str)
                .is_some()
                && value
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .is_some() =>
        {
            let state = value
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            (status_path, StatusRead::UnmodelledState(state))
        }
        _ => {
            let message = format!(
                "invalid async status beside manifest: {}",
                status_path.display()
            );
            (status_path, StatusRead::Error(message))
        }
    }
}

enum StatusRead {
    Absent,
    Parsed(Box<RunStatus>),
    UnmodelledState(String),
    Error(String),
}

/// pi `manifestMetadata` (`:387-420`) — the per-group, per-task join.
fn manifest_metadata(
    manifest_path: &Path,
    manifest: &Manifest,
    warnings: &mut BTreeSet<String>,
) -> Vec<MetadataRecord> {
    let mut records = Vec::new();
    if manifest.groups.len() > MAX_PLAN_ENTRIES {
        warnings.insert(format!(
            "handoff group discovery capped at {MAX_PLAN_ENTRIES} groups in {}",
            manifest_path.display()
        ));
    }
    let (status_path, status) = read_status_beside_manifest(manifest_path);
    for group in manifest.groups.iter().take(MAX_PLAN_ENTRIES) {
        if group.cleanup.tasks.len() > MAX_PLAN_ENTRIES {
            warnings.insert(format!(
                "cleanup task discovery capped at {MAX_PLAN_ENTRIES} tasks in {}",
                manifest_path.display()
            ));
        }
        for task in group.cleanup.tasks.iter().take(MAX_PLAN_ENTRIES) {
            if task.path.as_os_str().is_empty() || task.branch.trim().is_empty() {
                warnings.insert(format!(
                    "ignored malformed cleanup task in {}",
                    manifest_path.display()
                ));
                continue;
            }
            let mut matching = group
                .children
                .iter()
                .filter(|child| child.task_index == task.index);
            let Some(child) = matching.next() else {
                warnings.insert(format!(
                    "cleanup task {} does not have exactly one valid child metadata record in {}",
                    task.index,
                    manifest_path.display()
                ));
                continue;
            };
            if matching.next().is_some() {
                warnings.insert(format!(
                    "cleanup task {} does not have exactly one valid child metadata record in {}",
                    task.index,
                    manifest_path.display()
                ));
                continue;
            }
            records.push(MetadataRecord {
                manifest_path: manifest_path.to_path_buf(),
                manifest: manifest.clone(),
                group: group.clone(),
                task: task.clone(),
                child: child.clone(),
                status_path: status_path.clone(),
                status: match &status {
                    StatusRead::Parsed(status) => Some((**status).clone()),
                    _ => None,
                },
                status_raw_state: match &status {
                    StatusRead::UnmodelledState(state) => Some(state.clone()),
                    _ => None,
                },
                status_error: match &status {
                    StatusRead::Error(message) => Some(message.clone()),
                    _ => None,
                },
            });
        }
    }
    records
}

/// pi `loadMetadata` (`:422-441`).
pub(crate) async fn load_metadata(
    repo_root: &Path,
    explicit: Option<&Path>,
    extra: &[PathBuf],
) -> LoadedMetadata {
    let mut warnings = BTreeSet::new();
    let paths = discover_handoff_paths(repo_root, explicit, extra, &mut warnings);
    let explicit_key = explicit.map(|path| {
        comparable_path(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            crate::spawn::worktree::lexical_normalize(repo_root, path)
        })
    });

    let mut records = Vec::new();
    for manifest_path in &paths {
        match read_manifest_bounded(manifest_path).await {
            Ok(manifest) => {
                records.extend(manifest_metadata(manifest_path, &manifest, &mut warnings));
            }
            Err(error) => {
                // pi `:429`: a read failure is only WARNED about when the caller named this file
                // explicitly. A discovered file that turns out not to be a manifest is not the
                // operator's problem.
                if explicit_key
                    .as_deref()
                    .is_some_and(|key| comparable_path(manifest_path) == key)
                {
                    warnings.insert(error);
                }
            }
        }
    }
    LoadedMetadata {
        records,
        paths,
        warnings,
    }
}

/// pi `terminalRunState` (`:481-483`), exhaustive over cyrup's [`crate::background::RunState`].
///
/// Exhaustive on purpose: a seventh run state cannot be added to this build without this `match`
/// going red, which is the check pi's string comparison cannot make.
fn terminal_run_state(state: crate::background::RunState) -> bool {
    use crate::background::RunState;
    match state {
        RunState::Complete | RunState::Failed | RunState::Paused | RunState::Stopped => true,
        RunState::Queued | RunState::Running => false,
    }
}

/// Whether the owning run is provably finished — pi `inspectRunState` (`:485-519`).
pub(crate) enum RunStateInspection {
    /// The run is over and every child settled.
    Terminal,
    /// It is not.
    Active(ActiveReason),
    /// It aged out.
    Stale(StaleReason),
    /// It could not be established.
    Unknown(UnknownReason),
}

/// pi `inspectRunState` (`:485-519`), rung for rung.
pub(crate) async fn inspect_run_state(
    record: &MetadataRecord,
    now: i64,
    foreground_ownership: super::ForegroundOwnershipProbe<'_>,
) -> RunStateInspection {
    use crate::handoff::model::HandoffSource;

    if let Some(detail) = &record.status_error {
        return RunStateInspection::Unknown(UnknownReason::StatusUnreadable {
            detail: detail.clone(),
        });
    }
    if let Some(state) = &record.status_raw_state {
        return RunStateInspection::Unknown(UnknownReason::UnknownRunState {
            state: state.clone(),
        });
    }
    if let Some(status) = &record.status {
        if status.run_id.as_str() != record.manifest.run_id.as_str() {
            return RunStateInspection::Unknown(UnknownReason::StatusRunIdMismatch {
                status: status.run_id.as_str().to_string(),
                manifest: record.manifest.run_id.as_str().to_string(),
            });
        }
        match status.state {
            crate::background::RunState::Queued => {
                return RunStateInspection::Active(ActiveReason::OwningRunState {
                    state: "queued",
                });
            }
            crate::background::RunState::Running => {
                return RunStateInspection::Active(ActiveReason::OwningRunState {
                    state: "running",
                });
            }
            other if !terminal_run_state(other) => {
                // Unreachable today (the two non-terminal states are handled above), and kept as
                // a match arm rather than deleted so that adding a seventh RunState variant
                // lands here instead of silently reading as terminal.
                return RunStateInspection::Unknown(UnknownReason::UnknownRunState {
                    state: serde_json::to_value(other)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_else(|| format!("{other:?}")),
                });
            }
            _ => {}
        }
    } else if record.manifest.source == HandoffSource::Async {
        return RunStateInspection::Unknown(UnknownReason::AsyncStatusMissing {
            manifest_path: record.manifest_path.clone(),
        });
    }

    // pi `:493`. A group with ZERO children and at least one cleanup task is ACTIVE: an
    // allocated-but-unsettled worktree blocks removal.
    if record.group.children.is_empty()
        || !record
            .group
            .children
            .iter()
            .all(|child| child.status.is_terminal())
    {
        return RunStateInspection::Active(ActiveReason::NonTerminalChild);
    }

    if record.manifest.source == HandoffSource::Foreground {
        return match foreground_ownership(record.manifest.run_id.as_str()) {
            super::model::ForegroundRunOwnership::Active => {
                RunStateInspection::Active(ActiveReason::ForegroundRunActive)
            }
            super::model::ForegroundRunOwnership::Terminal => RunStateInspection::Terminal,
            super::model::ForegroundRunOwnership::Unknown => {
                RunStateInspection::Unknown(UnknownReason::ForegroundOwnershipNotProvable)
            }
        };
    }

    let marker_dir = record
        .status_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    match crate::background::active_run_index::active_run_marker_age_ms(&marker_dir, now).await {
        Some(age)
            if age
                <= crate::background::active_run_index::DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS =>
        {
            RunStateInspection::Active(ActiveReason::RecentActiveMarker)
        }
        Some(age) => RunStateInspection::Stale(StaleReason::ActiveMarkerStale {
            age_secs: age / 1000,
        }),
        None => RunStateInspection::Terminal,
    }
}
