//! Flat-ruleset evaluation (port of pi `evaluate-permission.ts`). Flatten all rulesets in order,
//! iterate **from the end**, and return the first (last-in-order) rule whose `tool` wildcard matches
//! `tool` AND whose `pattern` wildcard matches `command`. No match → `Ask` (pi `:57`). This is the
//! fold used by the session approval store and by the gate's config+session overlay
//! (`applyPatternApprovalState`, v0.8.0 pi `index.ts:557-579` — ruleset order `[config, session]`,
//! so session beats config on overlap).
//!
//! The v0.7.1 shape was three rulesets, `[config, session, permanent]`, cited here as
//! `index.ts:850-874`. v0.8.0 deleted `permanent-approval-store.ts` outright; see [`crate::stores`].

use crate::types::{PatternRule, PermissionState};
use crate::wildcard;

/// The outcome of [`evaluate`] (pi `PatternPermissionEvaluation`, `evaluate-permission.ts:10-14`).
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub action: PermissionState,
    pub matched_pattern: Option<String>,
    pub matched_tool: Option<String>,
}

/// pi `evaluatePermission` (`evaluate-permission.ts:31-58`). `rulesets` are concatenated in order;
/// the LAST matching rule (scanning from the end) wins.
#[must_use]
pub fn evaluate(tool: &str, command: &str, rulesets: &[&[PatternRule]]) -> Evaluation {
    let rules: Vec<&PatternRule> = rulesets.iter().flat_map(|rs| rs.iter()).collect();

    for index in (0..rules.len()).rev() {
        let Some(rule) = rules.get(index) else {
            continue;
        };
        let tool_pat = wildcard::compile(&rule.tool, ());
        if !tool_pat.is_match(tool) {
            continue;
        }
        let cmd_pat = wildcard::compile(&rule.pattern, ());
        if !cmd_pat.is_match(command) {
            continue;
        }
        return Evaluation {
            action: rule.action,
            matched_pattern: Some(rule.pattern.clone()),
            matched_tool: Some(rule.tool.clone()),
        };
    }

    Evaluation {
        action: PermissionState::Ask,
        matched_pattern: None,
        matched_tool: None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn rule(tool: &str, pattern: &str, action: PermissionState) -> PatternRule {
        PatternRule {
            tool: tool.into(),
            pattern: pattern.into(),
            action,
        }
    }

    #[test]
    fn no_match_is_ask() {
        let e = evaluate("bash", "ls", &[]);
        assert_eq!(e.action, PermissionState::Ask);
    }

    #[test]
    fn oversized_allow_rule_falls_through_to_ask_not_allow() {
        // Consumer-level proof that the 500-char cap (pi `wildcard-matcher.ts:16,21-27`) reads as
        // NEVER-match here, not match-everything. 501 `*`s escape to `.*`×501 without the cap, so
        // an uncapped build hands back `Allow` for an arbitrary command.
        let oversized = [rule("bash", &"*".repeat(501), PermissionState::Allow)];
        let e = evaluate("bash", "rm -rf /", &[&oversized]);
        assert_eq!(e.action, PermissionState::Ask);
        assert_eq!(e.matched_pattern, None);

        // Same for an oversized `tool` wildcard.
        let oversized_tool = [rule(&"*".repeat(501), "*", PermissionState::Allow)];
        let e = evaluate("bash", "rm -rf /", &[&oversized_tool]);
        assert_eq!(e.action, PermissionState::Ask);
    }

    #[test]
    fn rule_at_exactly_the_cap_still_allows() {
        // MIRROR CASE — pi's check is `> 500`; a 500-char pattern must keep working end-to-end.
        let at_cap = [rule("bash", &"*".repeat(500), PermissionState::Allow)];
        let e = evaluate("bash", "rm -rf /", &[&at_cap]);
        assert_eq!(e.action, PermissionState::Allow);
    }

    #[test]
    fn a_later_ruleset_beats_an_earlier_one_last_match_wins() {
        // `evaluate` folds an arbitrary NUMBER of rulesets and scans from the end, so the last
        // ruleset to match wins. The live gate passes two of them, `[config, session]` — this was
        // named `permanent_beats_session_beats_config` when a third `permanent` tier existed, but
        // v0.8.0 deleted that store and the fold itself never knew the tiers' names. Three rulesets
        // are kept here deliberately: they prove the ordering rule is general, not hardcoded to two.
        let config = [rule("bash", "*", PermissionState::Ask)];
        let session = [rule("bash", "git *", PermissionState::Allow)];
        let last = [rule("bash", "git *", PermissionState::Deny)];
        let e = evaluate("bash", "git push", &[&config, &session, &last]);
        assert_eq!(e.action, PermissionState::Deny);
    }
}
