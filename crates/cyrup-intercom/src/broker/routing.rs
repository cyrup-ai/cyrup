//! Session resolution + the ask-edge type (`broker.ts:44-48,581-596`).

/// An outstanding ask edge (`broker.ts:44-48`): `askEdges[message.id] = { from, to, createdAt }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskEdge {
    /// The session that issued the ask.
    pub from: String,
    /// The session the ask targets.
    pub to: String,
    /// Epoch-ms creation time (for the 10-minute prune).
    pub created_at: u64,
}

/// `findSessions` (`broker.ts:581-596`): resolve `name_or_id` to zero, one, or many session ids by
/// the fixed precedence — exact id, then case-insensitive exact name (may be multiple), then unique
/// id prefix. `entries` is `(id, name)` for every connected session.
#[must_use]
pub fn find_session_ids(entries: &[(String, Option<String>)], name_or_id: &str) -> Vec<String> {
    // 1. exact id.
    if entries.iter().any(|(id, _)| id == name_or_id) {
        return vec![name_or_id.to_string()];
    }
    // 2. case-insensitive exact name (may be multiple).
    let lower = name_or_id.to_lowercase();
    let by_name: Vec<String> = entries
        .iter()
        .filter(|(_, name)| name.as_deref().map(|n| n.to_lowercase()) == Some(lower.clone()))
        .map(|(id, _)| id.clone())
        .collect();
    if !by_name.is_empty() {
        return by_name;
    }
    // 3. id prefix.
    entries
        .iter()
        .filter(|(id, _)| id.starts_with(name_or_id))
        .map(|(id, _)| id.clone())
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
        assert_eq!(find_session_ids(&entries(), "abc123"), vec!["abc123".to_string()]);
    }

    #[test]
    fn case_insensitive_name_match() {
        assert_eq!(find_session_ids(&entries(), "alice"), vec!["abc123".to_string()]);
    }

    #[test]
    fn ambiguous_prefix_returns_multiple() {
        let mut got = find_session_ids(&entries(), "abc");
        got.sort();
        assert_eq!(got, vec!["abc123".to_string(), "abcdef".to_string()]);
    }

    #[test]
    fn unique_prefix_resolves() {
        assert_eq!(find_session_ids(&entries(), "zzz"), vec!["zzz999".to_string()]);
    }

    #[test]
    fn no_match_is_empty() {
        assert!(find_session_ids(&entries(), "nope").is_empty());
    }
}
