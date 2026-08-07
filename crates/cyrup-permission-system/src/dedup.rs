//! Permission-prompt de-duplication (port of pi `index.ts:707-798`, `143-144`, `1794-1902`). A
//! re-emitted IDENTICAL `tool_call` (same `toolCallId` + same fingerprint) must render ZERO
//! additional prompts: its prior decision is reused (any approval collapsed to a plain Allow-Once). A
//! NEW `toolCallId` produces a fresh, independent approval. Verified by
//! `edit-decision-deduplication-red.test.ts`.
//!
//! pi closes a SECOND window too: two CONCURRENT identical asks (the first prompt still open when the
//! second arrives). pi's `promptPermission` builds the `decisionPromise` for the real prompt and
//! `rememberPermissionPromptDecision`s it into the cache BEFORE awaiting it (`index.ts:1817-1892`,
//! esp. the register-then-await ordering at `:1888-1895`), so a concurrent duplicate hits the cache
//! and awaits the SAME still-pending promise instead of opening a second dialog
//! (`getCachedPermissionPromptDecision`, `index.ts:758-774`, called from `:1799-1815`). [`DedupCache`]
//! carries this via [`DedupCache::begin_pending`]/[`Lookup::Pending`]/[`Pending::wait`] — the caller
//! (the gate's `ask` path, `extension.rs`) must register the in-flight decision with
//! `begin_pending` BEFORE awaiting the human, mirroring pi's ordering exactly; [`Self::get`]/
//! [`Self::remember`] remain for a caller that only ever stores an already-resolved decision.
//!
//! Wired into `extension.rs`'s `prompt_decision` — the port of pi's `promptPermission`, and the
//! single place EVERY ask surface funnels through. pi puts the cache inside `promptPermission` itself
//! (`index.ts:1798-1815` lookup, `:1890-1892` store) rather than at any one call site, so all three
//! surfaces — skill-read (`index.ts:2282`), external-directory (`:2369`) and the main check
//! (`:2469`) — dedup identically; cyrup matches that placement. `prompt_decision` computes the cache
//! key from its [`DedupDetails`], checks the cache BEFORE invoking the [`crate::ask::AskChannel`],
//! and remembers the resolved decision after.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::sync::watch;

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

/// A cache slot: either an already-resolved decision, or one still in flight (pi's `decisionPromise`,
/// remembered before it settles — `index.ts:1817-1892`).
enum Slot {
    Ready(PermissionPromptDecision),
    Pending {
        tx: watch::Sender<Option<PermissionPromptDecision>>,
        /// Identity token so [`PendingOwner::resolve`]/[`PendingOwner::forget`] only ever touch the
        /// SAME registration they created — a later `remember`/`begin_pending` for the same key may
        /// have already replaced this entry (pi `forgetPermissionPromptDecision`'s
        /// `entry?.decisionPromise === decisionPromise` identity guard, `index.ts:795`).
        token: Arc<()>,
    },
}

struct Entry {
    key: String,
    cached_at: Instant,
    slot: Slot,
}

/// The result of a cache lookup (pi `getCachedPermissionPromptDecision`, `index.ts:758-774`).
pub enum Lookup {
    /// An already-resolved decision, collapsed exactly like a plain cache hit.
    Ready(PermissionPromptDecision),
    /// Another identical request's decision is still in flight — a genuine concurrent duplicate.
    /// Await [`Pending::wait`] instead of prompting again.
    Pending(Pending),
}

/// A FOLLOWER's handle on an in-flight decision (a second concurrent identical request that hit
/// [`Lookup::Pending`]).
#[derive(Clone)]
pub struct Pending {
    rx: watch::Receiver<Option<PermissionPromptDecision>>,
}

impl Pending {
    /// Await the owner's eventual decision, collapsed exactly like a resolved cache hit (pi
    /// `createDuplicatePermissionPromptDecision(await cachedDecision)`, `index.ts:1804`). If the
    /// owner's prompt never resolved (its [`PendingOwner`] was dropped via
    /// [`PendingOwner::forget`] without ever calling [`PendingOwner::resolve`]), fail CLOSED — there
    /// is no decision to reuse, so this never silently grants access.
    pub async fn wait(mut self) -> PermissionPromptDecision {
        loop {
            if let Some(decision) = self.rx.borrow().as_ref() {
                return create_duplicate_decision(decision);
            }
            if self.rx.changed().await.is_err() {
                return PermissionPromptDecision {
                    approved: false,
                    state: PermissionDecisionState::Reject,
                    denial_reason: None,
                };
            }
        }
    }
}

/// The OWNER's handle for an in-flight registration (pi's `decisionPromise` identity). The FIRST
/// caller for a cache key calls [`DedupCache::begin_pending`] to get one of these BEFORE awaiting the
/// real prompt (mirroring pi's `rememberPermissionPromptDecision` call ahead of `await
/// decisionPromise`, `index.ts:1890-1895`), then settles it with [`Self::resolve`] (success) or
/// [`Self::forget`] (the prompt errored, pi's `catch` branch, `index.ts:1896-1901`).
pub struct PendingOwner {
    key: String,
    tx: watch::Sender<Option<PermissionPromptDecision>>,
    token: Arc<()>,
}

impl PendingOwner {
    /// pi: the `decisionPromise` resolving successfully. If this registration is still the current
    /// entry for the key (it hasn't been superseded), settle it to `decision` so subsequent lookups
    /// see [`Lookup::Ready`]; every follower's [`Pending::wait`] wakes with the collapsed decision.
    pub fn resolve(self, cache: &mut DedupCache, decision: PermissionPromptDecision) {
        if let Some(entry) = cache.entries.iter_mut().find(|e| e.key == self.key) {
            let is_this_registration =
                matches!(&entry.slot, Slot::Pending { token, .. } if Arc::ptr_eq(token, &self.token));
            if is_this_registration {
                entry.slot = Slot::Ready(decision.clone());
            }
        }
        let _ = self.tx.send(Some(decision));
    }

    /// pi `forgetPermissionPromptDecision` (`index.ts:789-798`): drop the cache entry ONLY if it's
    /// still THIS registration (a later `remember`/`begin_pending` for the same key already replaced
    /// it — leave that one alone). Any follower already holding a [`Pending`] fails CLOSED once this
    /// owner's sender drops here without ever sending (see [`Pending::wait`]).
    pub fn forget(self, cache: &mut DedupCache) {
        cache.entries.retain(|e| {
            let is_this_registration =
                matches!(&e.slot, Slot::Pending { token, .. } if Arc::ptr_eq(token, &self.token));
            !(e.key == self.key && is_this_registration)
        });
        // `self.tx` drops here, unblocking any waiter's `changed()` with an error.
    }
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

    /// pi `getCachedPermissionPromptDecision` (`index.ts:758-774`): a live entry — resolved OR still
    /// in flight — is returned; an expired entry is dropped and yields `None`. Prefer this over
    /// [`Self::get`] for a caller that CAN await a [`Lookup::Pending`], so a concurrent identical
    /// request collapses to one prompt instead of missing the cache.
    pub fn lookup(&mut self, key: &str) -> Option<Lookup> {
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
        match self.entries.get(pos) {
            Some(Entry { slot: Slot::Ready(decision), .. }) => {
                Some(Lookup::Ready(create_duplicate_decision(decision)))
            }
            Some(Entry { slot: Slot::Pending { tx, .. }, .. }) => {
                Some(Lookup::Pending(Pending { rx: tx.subscribe() }))
            }
            None => None,
        }
    }

    /// pi `getCachedPermissionPromptDecision`, resolved-only variant: same as [`Self::lookup`] but a
    /// still-in-flight entry (a genuine concurrent duplicate) is treated as a miss rather than
    /// returned for a blocking wait. A synchronous caller that cannot await should use this; a caller
    /// that CAN await should use [`Self::lookup`] + [`Pending::wait`] instead, so concurrent
    /// duplicates collapse to one prompt (pi `index.ts:1798-1815` + `1890-1895`).
    pub fn get(&mut self, key: &str) -> Option<PermissionPromptDecision> {
        match self.lookup(key)? {
            Lookup::Ready(decision) => Some(decision),
            Lookup::Pending(_) => None,
        }
    }

    /// pi `rememberPermissionPromptDecision` (`index.ts:776-787`) with an ALREADY-resolved decision
    /// (no in-flight window): replace-then-insert-at-end, then prune (drop expired, evict oldest
    /// beyond the cap). Prefer [`Self::begin_pending`] + [`PendingOwner::resolve`] to close the
    /// concurrent-duplicate window pi closes by remembering the decision BEFORE it resolves
    /// (`index.ts:1890-1895`).
    pub fn remember(&mut self, key: &str, decision: PermissionPromptDecision) {
        self.entries.retain(|e| e.key != key);
        self.entries.push(Entry { key: key.to_string(), cached_at: Instant::now(), slot: Slot::Ready(decision) });
        self.prune();
    }

    /// pi `rememberPermissionPromptDecision` called with the STILL-PENDING `decisionPromise`
    /// (`index.ts:1890-1892`, BEFORE `await decisionPromise` at `:1895`): register `key` as in-flight
    /// so a concurrent identical request (`Self::lookup` hitting `Lookup::Pending`) awaits this SAME
    /// decision instead of opening a second dialog. Replace-then-insert-at-end, then prune, matching
    /// [`Self::remember`]. The caller MUST eventually settle the returned [`PendingOwner`] with
    /// [`PendingOwner::resolve`] or [`PendingOwner::forget`].
    pub fn begin_pending(&mut self, key: &str) -> PendingOwner {
        self.entries.retain(|e| e.key != key);
        let (tx, _rx) = watch::channel(None);
        let token = Arc::new(());
        self.entries.push(Entry {
            key: key.to_string(),
            cached_at: Instant::now(),
            slot: Slot::Pending { tx: tx.clone(), token: Arc::clone(&token) },
        });
        self.prune();
        PendingOwner { key: key.to_string(), tx, token }
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

    /// Regression proof for the concurrent-duplicate-prompt gap (pi `index.ts:1798-1815` +
    /// `1890-1895`: `rememberPermissionPromptDecision` is called with the still-PENDING
    /// `decisionPromise`, before it is awaited, so a second identical request arriving while the first
    /// prompt is still open reuses the SAME pending promise — exactly one dialog for two concurrent
    /// duplicate requests). Before `begin_pending`/`Lookup::Pending` existed, a second concurrent
    /// caller had no way to observe an in-flight decision at all: `get()` only ever sees an
    /// already-resolved entry, so two concurrent identical asks would each independently miss the
    /// cache and prompt — this test fails against that behavior (it needs the in-flight registration
    /// to exist and be observable BEFORE the owner resolves it).
    #[tokio::test]
    async fn concurrent_duplicate_awaits_same_pending_decision_collapsed() {
        let mut cache = DedupCache::new();
        let d = DedupDetails { request_id: "call-concurrent".into(), command: Some("rm -rf".into()), ..Default::default() };
        let key = d.cache_key().unwrap();

        assert!(cache.lookup(&key).is_none(), "cold miss");
        // First caller: owner, registers the in-flight decision BEFORE the prompt resolves.
        let owner = cache.begin_pending(&key);

        // Second, concurrent, IDENTICAL request: must observe the SAME in-flight decision (one
        // dialog), not a cache miss that would open a second one.
        let lookup = cache.lookup(&key);
        assert!(matches!(lookup, Some(Lookup::Pending(_))), "expected an in-flight (Pending) hit");
        let follower = match lookup {
            Some(Lookup::Pending(p)) => p,
            _ => unreachable!("checked above"),
        };

        // The first prompt settles as an "always" approval; the follower must observe it, collapsed
        // to a plain Allow-Once (never re-persisting an "always" grant for a re-emitted duplicate).
        let waiter = tokio::spawn(follower.wait());
        owner.resolve(&mut cache, approved_always());
        let collapsed = waiter.await.unwrap();
        assert!(collapsed.approved);
        assert_eq!(collapsed.state, PermissionDecisionState::Approved);

        // The now-settled entry also serves ordinary (non-concurrent) lookups as a normal cache hit.
        let hit = cache.get(&key).unwrap();
        assert!(hit.approved);
        assert_eq!(hit.state, PermissionDecisionState::Approved);
    }

    /// pi `forgetPermissionPromptDecision` (`index.ts:789-798`): if the real prompt errors, the entry
    /// is dropped rather than left dangling — and any follower awaiting it must NOT be silently
    /// granted access (fail-closed), since pi's `decisionPromise` rejection propagates to every
    /// awaiter instead of resolving.
    #[tokio::test]
    async fn pending_forgotten_on_error_fails_closed_for_waiters_and_clears_entry() {
        let mut cache = DedupCache::new();
        let d = DedupDetails { request_id: "call-err".into(), command: Some("y".into()), ..Default::default() };
        let key = d.cache_key().unwrap();

        let owner = cache.begin_pending(&key);
        let lookup = cache.lookup(&key);
        assert!(matches!(lookup, Some(Lookup::Pending(_))));
        let follower = match lookup {
            Some(Lookup::Pending(p)) => p,
            _ => unreachable!("checked above"),
        };

        let waiter = tokio::spawn(follower.wait());
        owner.forget(&mut cache);
        let decision = waiter.await.unwrap();
        assert!(!decision.approved);
        assert_eq!(decision.state, PermissionDecisionState::Reject);

        // A fresh request now starts a NEW prompt rather than reusing the failed one.
        assert!(cache.lookup(&key).is_none());
    }
}
