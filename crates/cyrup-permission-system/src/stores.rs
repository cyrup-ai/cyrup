//! The two approval stores (port of pi `session-approval-store.ts` + `permanent-approval-store.ts`).
//!
//! - **Session** store: in-memory, allow-only, per session; cleared on session start AND shutdown.
//!   `approveAlways == approveOnce` (both push an `allow` rule). This is the ONLY runtime sink for an
//!   `ask` "Allow Always" decision (pi `index.ts:905`).
//! - **Permanent** store: an on-disk flat JSON array `[{tool,pattern,action}]`, tri-state,
//!   read-through only at runtime. §8.2: the extension **never writes it** — the `approve_always`
//!   atomic writer is retained for source fidelity but is NOT wired into the "always" path (which
//!   goes to the session store). Malformed/absent → `[]` gracefully.

use std::path::PathBuf;

use crate::error::PermissionError;
use crate::types::{PatternRule, PermissionState};

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
}

/// pi `PermanentApprovalStore` — on-disk JSON array, tri-state, READ-THROUGH only.
#[derive(Debug)]
pub struct PermanentApprovalStore {
    persistence_path: PathBuf,
    rules: Option<Vec<PatternRule>>,
}

impl PermanentApprovalStore {
    #[must_use]
    pub fn new(persistence_path: PathBuf) -> Self {
        Self { persistence_path, rules: None }
    }

    fn ensure_loaded(&mut self) {
        if self.rules.is_none() {
            self.rules = Some(Self::load_rules(&self.persistence_path));
        }
    }

    /// pi `loadRules` (`permanent-approval-store.ts:68-85`): absent → `[]`; malformed JSON → `[]`;
    /// else keep only well-formed `{tool,pattern,action}` rules, trimmed.
    fn load_rules(path: &PathBuf) -> Vec<PatternRule> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        let Some(array) = value.as_array() else {
            return Vec::new();
        };
        array.iter().filter_map(Self::parse_persisted_rule).collect()
    }

    /// pi `isPersistedRule` + normalize (`permanent-approval-store.ts:6-21,76-80`).
    fn parse_persisted_rule(value: &serde_json::Value) -> Option<PatternRule> {
        let obj = value.as_object()?;
        let tool = obj.get("tool")?.as_str()?.trim();
        let pattern = obj.get("pattern")?.as_str()?.trim();
        if tool.is_empty() || pattern.is_empty() {
            return None;
        }
        let action = PermissionState::parse(obj.get("action")?.as_str()?)?;
        Some(PatternRule { tool: tool.to_string(), pattern: pattern.to_string(), action })
    }

    /// A clone of the loaded rules (pi `getRules`). Lazy-loads on first call.
    #[must_use]
    pub fn get_rules(&mut self) -> Vec<PatternRule> {
        self.ensure_loaded();
        self.rules.clone().unwrap_or_default()
    }

    /// pi `approveAlways` + `saveRules` (`permanent-approval-store.ts:37-52,87-92`): atomic
    /// `temp+rename` write of the full rules array. **RETAINED FOR SOURCE FIDELITY ONLY — NOT WIRED
    /// into the runtime "always" path** (§8.2: pi's sole runtime persistence sink is the SESSION
    /// store, `index.ts:905`). Kept so a future explicit "persist permanently" surface can reuse the
    /// exact on-disk format without re-deriving it.
    pub fn approve_always(
        &mut self,
        tool: &str,
        pattern: &str,
        action: PermissionState,
    ) -> Result<(), PermissionError> {
        let t = tool.trim();
        let p = pattern.trim();
        if t.is_empty() || p.is_empty() {
            return Ok(());
        }
        self.ensure_loaded();
        if let Some(rules) = self.rules.as_mut() {
            rules.push(PatternRule {
                tool: t.to_string(),
                pattern: p.to_string(),
                action,
            });
        }
        self.save_rules()
    }

    fn save_rules(&self) -> Result<(), PermissionError> {
        let rules = self.rules.clone().unwrap_or_default();
        if let Some(parent) = self.persistence_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let serializable: Vec<serde_json::Value> = rules
            .iter()
            .map(|r| {
                serde_json::json!({
                    "tool": r.tool,
                    "pattern": r.pattern,
                    "action": match r.action {
                        PermissionState::Allow => "allow",
                        PermissionState::Deny => "deny",
                        PermissionState::Ask => "ask",
                    },
                })
            })
            .collect();
        let body = format!(
            "{}\n",
            serde_json::to_string_pretty(&serializable).map_err(|e| PermissionError::Io(e.to_string()))?
        );
        let pid = std::process::id();
        let temp = self.persistence_path.with_extension(format!("{pid}.tmp"));
        std::fs::write(&temp, body)?;
        std::fs::rename(&temp, &self.persistence_path)?;
        Ok(())
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
    fn permanent_store_absent_and_malformed_yield_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut absent = PermanentApprovalStore::new(dir.path().join("nope.json"));
        assert!(absent.get_rules().is_empty());

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let mut malformed = PermanentApprovalStore::new(bad);
        assert!(malformed.get_rules().is_empty());
    }

    #[test]
    fn permanent_store_reads_tristate_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        std::fs::write(
            &path,
            r#"[
                {"tool":"bash","pattern":"git *","action":"allow"},
                {"tool":"bash","pattern":"rm *","action":"deny"},
                {"tool":"","pattern":"x","action":"allow"},
                {"tool":"edit","pattern":"*","action":"bogus"}
            ]"#,
        )
        .unwrap();
        let mut store = PermanentApprovalStore::new(path);
        let rules = store.get_rules();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].action, PermissionState::Allow);
        assert_eq!(rules[1].action, PermissionState::Deny);
    }
}
