//! The operator-facing render — pi `formatWorktreeCleanupPlan` (`:832-869` @`v0.68.0`).
//!
//! Two lines here are safety text, not decoration, and must never be dropped: the trailing
//! *"Plan-only mode: no worktrees or branches were removed."* (`:866`) and the branch section's
//! *"none (plan-only mode; no branch will be deleted)"* (`:855`). A human reading this output
//! must not come away believing something was deleted.

use super::model::{CleanupDecision, CleanupPlanEntry, CreatedCleanupPlan};

/// pi `formatPlanEntry` (`:832-835`).
fn format_entry(entry: &CleanupPlanEntry) -> String {
    let target = if entry.branch.is_empty() {
        entry.path.display().to_string()
    } else {
        format!("{} [{}]", entry.path.display(), entry.branch)
    };
    let reasons = if entry.reasons.is_empty() {
        "no reason recorded".to_string()
    } else {
        entry.reasons.join("; ")
    };
    format!("- {target}: {reasons}")
}

fn section(title: &str, entries: &[&CleanupPlanEntry], empty: &str, lines: &mut Vec<String>) {
    lines.push(title.to_string());
    if entries.is_empty() {
        lines.push(format!("- {empty}"));
    } else {
        lines.extend(entries.iter().map(|entry| format_entry(entry)));
    }
}

/// pi `formatWorktreeCleanupPlan` (`:837-869`).
#[must_use]
pub fn format_worktree_cleanup_plan(created: &CreatedCleanupPlan) -> String {
    let plan = &created.plan;
    let removable: Vec<&CleanupPlanEntry> = plan
        .entries
        .iter()
        .filter(|entry| entry.decision == CleanupDecision::Remove)
        .collect();
    let branches: Vec<&CleanupPlanEntry> = removable
        .iter()
        .copied()
        .filter(|entry| entry.will_delete_branch == Some(true))
        .collect();
    let kept: Vec<&CleanupPlanEntry> = plan
        .entries
        .iter()
        .filter(|entry| entry.decision == CleanupDecision::Keep)
        .collect();
    let unknown: Vec<&CleanupPlanEntry> = plan
        .entries
        .iter()
        .filter(|entry| entry.decision == CleanupDecision::Unknown)
        .collect();

    let mut lines = vec![
        format!("Worktree cleanup plan {}", plan.plan_id),
        format!("Repository: {}", plan.repo_root.display()),
        format!("Expires: {}", format_epoch_millis(plan.expires_at)),
        format!("Entries: {}", plan.entries.len()),
        String::new(),
    ];
    section("Will remove", &removable, "none", &mut lines);
    lines.push(String::new());
    section(
        "Will delete local branches",
        &branches,
        "none (plan-only mode; no branch will be deleted)",
        &mut lines,
    );
    lines.push(String::new());
    section("Will keep, with reasons", &kept, "none", &mut lines);
    lines.push(String::new());
    section("Unknown, needs manual review", &unknown, "none", &mut lines);
    lines.push(String::new());
    lines.push("Prune candidates".to_string());
    if plan.prune_candidates.is_empty() {
        lines.push("- none (plan-only mode does not prune Git metadata)".to_string());
    } else {
        lines.extend(
            plan.prune_candidates
                .iter()
                .map(|candidate| format!("- {}", candidate.display())),
        );
    }
    if let Some(warnings) = plan.warnings.as_ref().filter(|w| !w.is_empty()) {
        lines.push(String::new());
        lines.push("Warnings".to_string());
        lines.extend(warnings.iter().map(|warning| format!("- {warning}")));
    }
    lines.push(String::new());
    lines.push(format!("Plan saved: {}", created.plan_path.display()));
    lines.push("Plan-only mode: no worktrees or branches were removed.".to_string());
    lines.join("\n")
}

/// `new Date(ms).toISOString()` (pi `:843`), hand-rolled.
///
/// Neither `time` nor `chrono` is a dependency of this crate — the sibling
/// [`crate::handoff::model`]'s `AttestationTimestamp` records the same finding — and one
/// human-readable expiry line does not justify adding one. The civil-date conversion is the
/// standard days-from-epoch algorithm, so it is exact for every value an `i64` epoch-millis can
/// hold rather than being approximate near leap years.
fn format_epoch_millis(millis: i64) -> String {
    let (days, ms_of_day) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    let seconds = ms_of_day / 1000;
    let (hour, minute, second) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        ms_of_day % 1000
    )
}

/// Howard Hinnant's `civil_from_days`, the algorithm every date library uses.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;

    #[test]
    fn epoch_millis_render_matches_javascript_to_iso_string() {
        // `new Date(0).toISOString()` and `new Date(1758153600000).toISOString()`.
        assert_eq!(format_epoch_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_epoch_millis(1_758_153_600_000),
            "2025-09-18T00:00:00.000Z"
        );
        // A leap day, which an approximate conversion gets wrong.
        assert_eq!(
            format_epoch_millis(1_709_164_800_000),
            "2024-02-29T00:00:00.000Z"
        );
    }
}
