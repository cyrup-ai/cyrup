//! Session resolution + the ask-edge type (`broker.ts:44-48,581-596`), keyed by
//! [`SessionKey`] since ICOM-055.

use crate::transport::protocol::ScopeId;

/// `scopedSessionKey(scopeId, sessionId)` (`v0.13.0 broker/broker.ts:148-150`) as a TYPE rather
/// than upstream's `JSON.stringify([scopeId ?? null, sessionId])` string.
///
/// A session id is unique only WITHIN a scope; the broker's identity for a session is the pair.
/// Deriving `Hash`/`Eq` gives `sameScope` (`:144-146`, a plain `===` over `string | undefined`) for
/// free and makes `None` a scope like any other — which is exactly why an unscoped session can only
/// ever reach unscoped peers, and why the absent-scope path stays bit-for-bit today's behaviour.
///
/// Upstream stringifies because a JS `Map` keys by reference; Rust does not need that indirection,
/// and skipping it is what makes a scope-less lookup a **compile** error rather than a silent miss:
/// a bare `String` session id is no longer a key of `BrokerState::sessions`,
/// `BrokerState::disconnected_sessions`, [`AskEdge`], `MessageReceiptRoute` or `MailboxMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    /// The registered scope, `None` for unscoped (`normalizeScopeId`,
    /// `v0.13.0 broker/broker.ts:133-142`).
    pub scope: Option<ScopeId>,
    /// The session id, unique only within [`Self::scope`].
    pub id: String,
}

impl SessionKey {
    /// The key for `id` in `scope`.
    #[must_use]
    pub fn new(scope: Option<ScopeId>, id: String) -> Self {
        Self { scope, id }
    }

    /// The key for `id` in the unscoped class — the scope every session registered into before
    /// ICOM-055, and the only one a client-side roster (which never sees a scope) can name.
    #[must_use]
    pub fn unscoped(id: String) -> Self {
        Self { scope: None, id }
    }

    /// `sameScope(this.scopeId, scope)` (`v0.13.0 broker/broker.ts:144-146`).
    #[must_use]
    pub fn in_scope(&self, scope: Option<&ScopeId>) -> bool {
        self.scope.as_ref() == scope
    }
}

/// An outstanding ask edge (`broker.ts:44-48`): `askEdges[message.id] = { from, to, createdAt }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskEdge {
    /// The session that issued the ask.
    pub from: SessionKey,
    /// The session the ask targets.
    pub to: SessionKey,
    /// Epoch-ms creation time (for the 10-minute prune).
    pub created_at: u64,
}

/// `findSessions` (`v0.13.0 broker/broker.ts:1247-1262`): resolve `name_or_id` **within `scope`** to
/// zero, one, or many sessions by the fixed precedence — exact id, then case-insensitive exact name
/// (may be multiple), then id prefix. `entries` is `(key, name)` for every session in the map.
///
/// EVERY tier filters on scope, so a peer in another scope is not merely unaddressable: it is
/// indistinguishable from a name the broker has never seen, and the caller's existing
/// `Session not found` refusal is the whole cross-scope answer. Nothing new leaks.
///
/// The prefix tier matches on `key.id` rather than on the map key — upstream made the same change
/// (`.filter(([, session]) => … session.info.id.startsWith(nameOrId))`, `:1260`) once its key
/// stopped being the bare id.
#[must_use]
pub fn find_session_keys(
    entries: &[(SessionKey, Option<String>)],
    name_or_id: &str,
    scope: Option<&ScopeId>,
) -> Vec<SessionKey> {
    // 1. exact id.
    if let Some((key, _)) = entries
        .iter()
        .find(|(k, _)| k.in_scope(scope) && k.id == name_or_id)
    {
        return vec![key.clone()];
    }
    // 2. case-insensitive exact name (may be multiple).
    let lower = name_or_id.to_lowercase();
    let by_name: Vec<SessionKey> = entries
        .iter()
        .filter(|(k, name)| {
            k.in_scope(scope)
                && name.as_deref().map(str::to_lowercase).as_deref() == Some(lower.as_str())
        })
        .map(|(k, _)| k.clone())
        .collect();
    if !by_name.is_empty() {
        return by_name;
    }
    // 3. id prefix.
    entries
        .iter()
        .filter(|(k, _)| k.in_scope(scope) && k.id.starts_with(name_or_id))
        .map(|(k, _)| k.clone())
        .collect()
}

/// [`find_session_keys`] over a roster that carries no scope at all — the CLIENT side, where
/// `crate::outbox::resolve_outbox_target` resolves a target against the `sessions` reply the broker
/// has already filtered (`resolveOutboxTarget`, `v0.12.0 index.ts:1029-1046`).
///
/// A client is never asked to filter its own view (ICOM-055): every id it can see is already in its
/// own scope, so the ladder runs in the unscoped class and the scope never reaches this layer.
#[must_use]
pub fn find_session_ids(entries: &[(String, Option<String>)], name_or_id: &str) -> Vec<String> {
    let keyed: Vec<(SessionKey, Option<String>)> = entries
        .iter()
        .map(|(id, name)| (SessionKey::unscoped(id.clone()), name.clone()))
        .collect();
    find_session_keys(&keyed, name_or_id, None)
        .into_iter()
        .map(|k| k.id)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn entries() -> Vec<(String, Option<String>)> {
        vec![
            ("abc123".to_string(), Some("Alice".to_string())),
            ("abcdef".to_string(), Some("Bob".to_string())),
            ("zzz999".to_string(), None),
        ]
    }

    #[test]
    fn exact_id_wins() {
        assert_eq!(
            find_session_ids(&entries(), "abc123"),
            vec!["abc123".to_string()]
        );
    }

    #[test]
    fn case_insensitive_name_match() {
        assert_eq!(
            find_session_ids(&entries(), "alice"),
            vec!["abc123".to_string()]
        );
    }

    #[test]
    fn ambiguous_prefix_returns_multiple() {
        let mut got = find_session_ids(&entries(), "abc");
        got.sort();
        assert_eq!(got, vec!["abc123".to_string(), "abcdef".to_string()]);
    }

    #[test]
    fn unique_prefix_resolves() {
        assert_eq!(
            find_session_ids(&entries(), "zzz"),
            vec!["zzz999".to_string()]
        );
    }

    #[test]
    fn no_match_is_empty() {
        assert!(find_session_ids(&entries(), "nope").is_empty());
    }

    fn scope(s: &str) -> Option<ScopeId> {
        ScopeId::parse(s)
    }

    fn scoped_entries() -> Vec<(SessionKey, Option<String>)> {
        vec![
            (
                SessionKey::new(scope("alpha"), "abc123".to_string()),
                Some("Alice".to_string()),
            ),
            (
                SessionKey::new(scope("beta"), "abcdef".to_string()),
                Some("Alice".to_string()),
            ),
            (
                SessionKey::unscoped("abc999".to_string()),
                Some("Alice".to_string()),
            ),
        ]
    }

    /// ICOM-055 — every tier of `findSessions` filters on scope
    /// (`v0.13.0 broker/broker.ts:1247-1262`). Before the port the ladder saw the whole roster, so
    /// each of these three lookups resolved a peer in another scope.
    #[test]
    fn every_resolution_tier_is_scoped() {
        let entries = scoped_entries();
        let alpha = scope("alpha");
        // Exact id.
        assert_eq!(
            find_session_keys(&entries, "abcdef", alpha.as_ref()),
            Vec::new(),
            "a full id from another scope resolves to nothing"
        );
        assert_eq!(
            find_session_keys(&entries, "abc123", alpha.as_ref()),
            vec![SessionKey::new(alpha.clone(), "abc123".to_string())]
        );
        // Exact name — the same name exists in all three classes.
        assert_eq!(
            find_session_keys(&entries, "alice", alpha.as_ref()),
            vec![SessionKey::new(alpha.clone(), "abc123".to_string())],
            "a name shared across scopes resolves only within the caller's"
        );
        // Id prefix.
        assert_eq!(
            find_session_keys(&entries, "abc", scope("beta").as_ref()),
            vec![SessionKey::new(scope("beta"), "abcdef".to_string())]
        );
    }

    /// Unscoped is a SCOPE, not a wildcard: `sameScope(undefined, "alpha")` is `false`
    /// (`v0.13.0 broker/broker.ts:144-146`), so an unscoped caller sees only unscoped peers and a
    /// scoped caller never sees the unscoped ones.
    #[test]
    fn unscoped_is_a_scope_and_not_a_wildcard() {
        let entries = scoped_entries();
        assert_eq!(
            find_session_keys(&entries, "alice", None),
            vec![SessionKey::unscoped("abc999".to_string())]
        );
        assert_eq!(
            find_session_keys(&entries, "abc999", scope("alpha").as_ref()),
            Vec::new()
        );
    }
}
