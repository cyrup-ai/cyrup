//! Flat-ruleset evaluation (port of pi `evaluate-permission.ts`). Flatten all rulesets in order,
//! iterate **from the end**, and return the first (last-in-order) rule whose `tool` wildcard matches
//! `tool` AND whose `pattern` wildcard matches `command`. No match → `Ask` (pi `:57`). This is the
//! fold used by both approval stores and by the gate's config+session+permanent overlay
//! (`applyPatternApprovalState`, pi `index.ts:850-874` — ruleset order `[config, session,
//! permanent]`, so permanent beats session beats config on overlap).

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
        let Some(rule) = rules.get(index) else { continue };
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

    Evaluation { action: PermissionState::Ask, matched_pattern: None, matched_tool: None }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn rule(tool: &str, pattern: &str, action: PermissionState) -> PatternRule {
        PatternRule { tool: tool.into(), pattern: pattern.into(), action }
    }

    #[test]
    fn no_match_is_ask() {
        let e = evaluate("bash", "ls", &[]);
        assert_eq!(e.action, PermissionState::Ask);
    }

    #[test]
    fn permanent_beats_session_beats_config_last_match_wins() {
        let config = [rule("bash", "*", PermissionState::Ask)];
        let session = [rule("bash", "git *", PermissionState::Allow)];
        let permanent = [rule("bash", "git *", PermissionState::Deny)];
        // ruleset order [config, session, permanent]; permanent's later deny wins.
        let e = evaluate("bash", "git push", &[&config, &session, &permanent]);
        assert_eq!(e.action, PermissionState::Deny);
    }
}
