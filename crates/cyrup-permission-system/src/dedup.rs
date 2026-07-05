//! Permission-prompt de-duplication (port of pi `index.ts:707-798`, `143-144`). A re-emitted
//! IDENTICAL `tool_call` (same `toolCallId` + same fingerprint) must render ZERO additional prompts:
//! its prior decision is reused (any approval collapsed to a plain Allow-Once). A NEW `toolCallId`
//! produces a fresh, independent approval. Verified by `edit-decision-deduplication-red.test.ts`.
//!
//! Wired into the gate's `ask` path (`extension.rs`): the gate computes the cache key, checks
//! [`DedupCache::get`] BEFORE invoking the [`crate::ask::AskChannel`], and [`DedupCache::remember`]s
//! the resolved decision after.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::ask::{PermissionDecisionState, PermissionPromptDecision};

/// pi `DUPLICATE_PERMISSION_PROMPT_CACHE_TTL_MS = 2 * 60 * 1000` (`index.ts:143`).
const CACHE_TTL: Duration = Duration::from_secs(2 * 60);
/// pi `DUPLICATE_PERMISSION_PROMPT_CACHE_MAX_ENTRIES = 128` (`index.ts:144`).
const CACHE_MAX_ENTRIES: usize = 128;

/// The fingerprint inputs (pi `PermissionPromptDetails` fields hashed at `index.ts:713-726`). Only
/// the fields pi hashes are carried; `request_id` is the `toolCallId`.
#[derive(Debug, Clone, Default)]
pub struct DedupDetails {
    pub request_id: String,
    pub source: String,
    pub agent_name: Option<String>,
    pub message: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub skill_name: Option<String>,
    pub path: Option<String>,
    pub command: Option<String>,
    pub target: Option<String>,
    pub tool_input: serde_json::Value,
}

impl DedupDetails {
    /// pi `createPermissionPromptCacheKey` (`index.ts:728-737`): `requestId \0 sha256(fingerprint)`,
    /// or `None` when `requestId` is empty (uncacheable). The fingerprint need only be internally
    /// consistent (dedup is per-process); the exact bytes need not match pi's.
    #[must_use]
    pub fn cache_key(&self) -> Option<String> {
        let request_id = self.request_id.trim();
        if request_id.is_empty() {
            return None;
        }
        let fingerprint = serde_json::json!({
            "source": self.source,
            "agentName": self.agent_name,
            "message": self.message,
            "toolCallId": self.tool_call_id,
            "toolName": self.tool_name,
            "skillName": self.skill_name,
            "path": self.path,
            "command": self.command,
            "target": self.target,
            "toolInput": self.tool_input,
        })
        .to_string();
        let mut hasher = Sha256::new();
        hasher.update(fingerprint.as_bytes());
        let hash = hasher.finalize();
        let hex = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
        Some(format!("{request_id}\u{0}{hex}"))
    }
}

struct Entry {
    key: String,
    cached_at: Instant,
    decision: PermissionPromptDecision,
}

/// The bounded, TTL'd decision cache. Insertion-ordered `Vec` (front = oldest) so eviction is O(1)
/// from the front, matching pi's `Map` iteration-order eviction (`index.ts:749-755`).
#[derive(Default)]
pub struct DedupCache {
    entries: Vec<Entry>,
}

impl DedupCache {
    #[must_use]
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// pi `getCachedPermissionPromptDecision` (`index.ts:758-774`): a live entry's decision, cloned
    /// and collapsed via [`create_duplicate_decision`]; an expired entry is dropped and yields
    /// `None`.
    pub fn get(&mut self, key: &str) -> Option<PermissionPromptDecision> {
        let now = Instant::now();
        let pos = self.entries.iter().position(|e| e.key == key)?;
        let expired = self
            .entries
            .get(pos)
            .map(|e| now.duration_since(e.cached_at) > CACHE_TTL)
            .unwrap_or(true);
        if expired {
            self.entries.remove(pos);
            return None;
        }
        self.entries.get(pos).map(|e| create_duplicate_decision(&e.decision))
    }

    /// pi `rememberPermissionPromptDecision` (`index.ts:776-787`): replace-then-insert-at-end, then
    /// prune (drop expired, evict oldest beyond the cap).
    pub fn remember(&mut self, key: &str, decision: PermissionPromptDecision) {
        self.entries.retain(|e| e.key != key);
        self.entries.push(Entry { key: key.to_string(), cached_at: Instant::now(), decision });
        self.prune();
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.entries.retain(|e| now.duration_since(e.cached_at) <= CACHE_TTL);
        while self.entries.len() > CACHE_MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// Clear all entries (session_start / session_shutdown housekeeping).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// pi `createDuplicatePermissionPromptDecision` (`index.ts:707-711`): an approval collapses to a
/// plain Allow-Once (so a re-emitted approval never re-persists an "always" grant); a rejection is
/// cloned as-is.
#[must_use]
pub fn create_duplicate_decision(decision: &PermissionPromptDecision) -> PermissionPromptDecision {
    if decision.approved {
        PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Approved,
            denial_reason: None,
        }
    } else {
        decision.clone()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn approved_always() -> PermissionPromptDecision {
        PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Always,
            denial_reason: None,
        }
    }

    #[test]
    fn same_key_reuses_collapsed_decision_new_key_misses() {
        let mut cache = DedupCache::new();
        let d1 = DedupDetails { request_id: "call-1".into(), command: Some("x".into()), ..Default::default() };
        let k1 = d1.cache_key().unwrap();
        assert!(cache.get(&k1).is_none(), "cold miss");
        cache.remember(&k1, approved_always());
        // Same id+fingerprint → hit, collapsed to Allow-Once (Approved), not Always.
        let hit = cache.get(&k1).unwrap();
        assert!(hit.approved);
        assert_eq!(hit.state, PermissionDecisionState::Approved);
        // A different toolCallId → fresh, independent (miss).
        let d2 = DedupDetails { request_id: "call-2".into(), command: Some("x".into()), ..Default::default() };
        let k2 = d2.cache_key().unwrap();
        assert!(cache.get(&k2).is_none());
    }

    #[test]
    fn empty_request_id_is_uncacheable() {
        let d = DedupDetails { request_id: "  ".into(), ..Default::default() };
        assert!(d.cache_key().is_none());
    }
}
