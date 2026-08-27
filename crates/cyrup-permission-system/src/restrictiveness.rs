//! Most-restrictive selection (port of pi `restrictiveness.ts:13-45`).

use crate::types::{PermissionCheckResult, PermissionState};

/// Restrictiveness ordering — `deny` > `ask` > `allow` (pi `RESTRICTIVENESS`, `:44-48`).
fn rank(state: PermissionState) -> u8 {
    match state {
        PermissionState::Allow => 0,
        PermissionState::Ask => 1,
        PermissionState::Deny => 2,
    }
}

/// The most restrictive result, **first wins on ties** (pi `pickMostRestrictive`, `:13-20`).
///
/// The winner is the offending candidate's OWN result, so `matched_pattern`, `command` and `source`
/// all name the input that forced the verdict rather than a synthesized composite. Passing
/// candidates in source order therefore yields the earliest worst case.
#[must_use]
pub fn pick_most_restrictive(results: Vec<PermissionCheckResult>) -> Option<PermissionCheckResult> {
    results.into_iter().reduce(|worst, next| {
        // Strictly greater, so the FIRST of equal-restrictiveness candidates is retained.
        if rank(next.state) > rank(worst.state) { next } else { worst }
    })
}
