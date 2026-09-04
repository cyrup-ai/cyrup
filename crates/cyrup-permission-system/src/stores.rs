//! The session approval store (port of pi `session-approval-store.ts`).
//!
//! In-memory, allow-only, per session; cleared on session start AND shutdown.
//! `approveAlways == approveOnce` (both push an `allow` rule). This is the ONLY approval sink the
//! runtime has (pi v0.8.0 `index.ts:595-612`, `persistSessionApprovalDecision`), and nothing it
//! records survives the process.
//!
//! # `PermanentApprovalStore` is gone (v0.7.1 → v0.8.0)
//!
//! Upstream deleted `src/permanent-approval-store.ts` outright in commit `a33ac2c`
//! (`feat(permissions)!: remove permanent approval store`, released as v0.8.0). There is no
//! replacement file and no replacement mechanism: v0.8.0's CHANGELOG `### Removed` states that
//! "`Allow Always` now records session-only (in-memory) approvals via `SessionApprovalStore`...
//! Cross-session persistent approvals are no longer written to disk." The only cross-session state
//! left is operator-authored policy (`cyrup-permissions.jsonc` + the extension `config.json`).
//!
//! The store was already write-dead here (and upstream at v0.7.1 — `PermanentApprovalStore.
//! approveAlways` had zero call sites in `index.ts`), so the OBSERVABLE consequence of the deletion
//! is narrow and exact: a hand-authored or legacy `cyrup-permission-system-approvals.json` on disk
//! no longer influences any decision. It used to rank LAST in the last-match-wins ruleset
//! (`[config, session, permanent]`, v0.7.1 `index.ts:850-874`), so it could override both the
//! session store and the operator's config rule — including with a `deny`, since unlike the
//! allow-only session store it was tri-state. That whole override tier is removed;
//! `evaluate` now sees `[config, session]` only (v0.8.0 `index.ts:557-579`).

use crate::types::{PatternRule, PermissionState};

/// The result of [`SessionApprovalStore::evaluate`] — pi's `{state, matchedPattern}`
/// (`session-approval-store.ts:28-33`). `matchedPattern` is dropped (set to `None`) whenever the
/// state isn't `allow`, exactly as pi's ternary does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvaluation {
    pub state: PermissionState,
    pub matched_pattern: Option<String>,
}

/// pi `SessionApprovalStore` — in-memory, allow-only.
#[derive(Debug, Default)]
pub struct SessionApprovalStore {
    rules: Vec<PatternRule>,
}

impl SessionApprovalStore {
    #[must_use]
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// pi `approveAlways`/`approveOnce` (`session-approval-store.ts:6-22`): trim; skip if either is
    /// empty; else push an `allow` rule.
    pub fn approve_always(&mut self, tool: &str, pattern: &str) {
        let t = tool.trim();
        let p = pattern.trim();
        if t.is_empty() || p.is_empty() {
            return;
        }
        self.rules.push(PatternRule {
            tool: t.to_string(),
            pattern: p.to_string(),
            action: PermissionState::Allow,
        });
    }

    /// A clone of the current rules (pi `getRules`).
    #[must_use]
    pub fn get_rules(&self) -> Vec<PatternRule> {
        self.rules.clone()
    }

    /// Clear all rules (pi `clear`; called on session_start + session_shutdown, `index.ts:2089,2123`).
    pub fn clear(&mut self) {
        self.rules.clear();
    }

    /// pi `evaluate` (`session-approval-store.ts:28-33`): evaluate `tool`/`command` against ONLY this
    /// store's own rules. `allow` keeps `matchedPattern`; anything else collapses to `ask` with
    /// `matchedPattern` dropped.
    #[must_use]
    pub fn evaluate(&self, tool: &str, command: &str) -> SessionEvaluation {
        let result = crate::evaluate::evaluate(tool, command, &[&self.rules]);
        if result.action == PermissionState::Allow {
            SessionEvaluation {
                state: PermissionState::Allow,
                matched_pattern: result.matched_pattern,
            }
        } else {
            SessionEvaluation {
                state: PermissionState::Ask,
                matched_pattern: None,
            }
        }
    }

    /// pi `hasSessionApproval` (`session-approval-store.ts:24-26`): `true` iff this store's own
    /// rules resolve `tool`/`command` to `allow`.
    #[must_use]
    pub fn has_session_approval(&self, tool: &str, command: &str) -> bool {
        self.evaluate(tool, command).state == PermissionState::Allow
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn session_store_is_allow_only_and_clears() {
        let mut s = SessionApprovalStore::new();
        s.approve_always("bash", "git *");
        s.approve_always("  ", "x"); // skipped (empty tool)
        let rules = s.get_rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, PermissionState::Allow);
        s.clear();
        assert!(s.get_rules().is_empty());
    }

    #[test]
    fn session_store_evaluate_and_has_session_approval() {
        let mut s = SessionApprovalStore::new();
        // No rules yet: evaluate() must resolve to Ask with no matched pattern, and
        // has_session_approval() must be false.
        let ask = s.evaluate("bash", "git push");
        assert_eq!(ask.state, PermissionState::Ask);
        assert_eq!(ask.matched_pattern, None);
        assert!(!s.has_session_approval("bash", "git push"));

        s.approve_always("bash", "git *");
        let allow = s.evaluate("bash", "git push");
        assert_eq!(allow.state, PermissionState::Allow);
        assert_eq!(allow.matched_pattern.as_deref(), Some("git *"));
        assert!(s.has_session_approval("bash", "git push"));

        // A non-matching command still resolves to Ask with no matched pattern.
        let no_match = s.evaluate("bash", "rm -rf /");
        assert_eq!(no_match.state, PermissionState::Ask);
        assert_eq!(no_match.matched_pattern, None);
        assert!(!s.has_session_approval("bash", "rm -rf /"));
    }
}
