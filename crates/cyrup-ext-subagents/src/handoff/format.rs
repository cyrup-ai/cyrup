//! Model-facing renderings — pi `formatStoredParallelHandoffCleanup` (`:411-430`),
//! `formatParallelHandoffReference` (`:676-678`) and `formatParallelHandoffError` (`:680-682`)
//! @v0.68.0. Every string below is upstream's verbatim.

use std::path::Path;

use super::error::HandoffError;
use super::evidence::trusted_cleanup_eligibility;
use super::model::{CleanupEligibility, HandoffReference, Manifest};

/// pi `firstRepositoryRoot` (`:402-405`) + `formatCleanupCommand` (`:407-410`).
fn cleanup_command(manifest_path: &Path, manifest: &Manifest) -> Option<String> {
    let repo_root = manifest
        .groups
        .first()
        .map(|group| group.repo_root.to_string_lossy().into_owned())
        .filter(|root| !root.trim().is_empty())?;
    Some(format!(
        "subagent({{ action: \"worktree.cleanup\", repo: {}, handoffPath: {}, mode: \"plan\" }})",
        json_string(&repo_root),
        json_string(&manifest_path.to_string_lossy())
    ))
}

/// pi's `JSON.stringify(<string>)` — the quoting the rendered command depends on.
fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// pi `formatStoredParallelHandoffCleanup` (`:411-430`).
///
/// `manifest` is `None` when the caller has not already read one (or the read threw): upstream
/// re-reads and swallows the error, and the missing/invalid case renders the SAME
/// "removal is not safe" prose an `unknown` verdict does.
#[must_use]
pub fn format_stored_cleanup(manifest_path: &Path, manifest: Option<&Manifest>) -> String {
    let Some(stored) = manifest else {
        return [
            "Cleanup eligibility: unknown".to_string(),
            "Reason: lane manifest is missing or invalid; removal is not safe.".to_string(),
            format!(
                "Plan command: subagent({{ action: \"worktree.cleanup\", handoffPath: {}, mode: \"plan\" }})",
                json_string(&manifest_path.to_string_lossy())
            ),
        ]
        .join("\n");
    };
    let eligibility = trusted_cleanup_eligibility(stored);
    let mut lines = vec![format!(
        "Cleanup eligibility: {}",
        eligibility_word(&eligibility)
    )];
    match &eligibility {
        CleanupEligibility::TerminalBlocked { reason } => {
            lines.push(format!("Reason: {reason}"));
        }
        CleanupEligibility::Unknown => {
            lines.push(
                "Reason: stored eligibility is missing or invalid; removal is not safe."
                    .to_string(),
            );
        }
        CleanupEligibility::Active => {
            lines.push("Reason: a child owner is still active; removal is not safe.".to_string());
        }
        CleanupEligibility::TerminalEligible | CleanupEligibility::SupersededEligible => {}
    }
    if let Some(command) = cleanup_command(manifest_path, stored) {
        lines.push(format!("Plan command: {command}"));
    }
    lines.join("\n")
}

/// The on-disk `state` word, which is also what upstream interpolates into the first line.
fn eligibility_word(eligibility: &CleanupEligibility) -> &'static str {
    match eligibility {
        CleanupEligibility::Active => "active",
        CleanupEligibility::TerminalEligible => "terminal-eligible",
        CleanupEligibility::TerminalBlocked { .. } => "terminal-blocked",
        CleanupEligibility::SupersededEligible => "superseded-eligible",
        CleanupEligibility::Unknown => "unknown",
    }
}

/// pi `formatParallelHandoffReference` (`:676-678`) — the line appended to a fan-out's output so
/// the caller can see, and address, the manifest.
#[must_use]
pub fn format_reference(reference: &HandoffReference) -> String {
    format!(
        "Worktree handoff: {} ({} children, {} changed patches, cleanup {})",
        reference.path.display(),
        reference.child_count,
        reference.changed_patches,
        match reference.cleanup_state {
            super::model::CleanupState::Complete => "complete",
            super::model::CleanupState::Partial => "partial",
        }
    )
}

/// pi `formatParallelHandoffError` (`:680-682`).
///
/// A manifest-write failure must never fail the fan-out itself: the children already ran and
/// their patches are already on disk. Upstream appends this line to the output and continues.
#[must_use]
pub fn format_error(error: &HandoffError) -> String {
    format!("Worktree handoff unavailable: {error}")
}
