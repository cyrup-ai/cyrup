//! RunId (func-SA §4.5, R-SA-072)
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

/// An opaque run-id token minted at spawn time — a short, URL/filesystem-safe random token with
/// **no pre-flight uniqueness check** against the filesystem or any registry (R-SA-072): the
/// child run directory's existence is established later, via `mkdir` at spawn time, not by this
/// type.
///
/// # Entropy
///
/// func-SA §4.5 illustratively targets an "8-hex-character random token"; arch-SA §3.6's sketch
/// widens that to "8+ char". Neither figure is itself the safety property — the actual
/// requirement is that two runs minted concurrently (including across process boundaries, e.g.
/// two orchestrator instances on the same machine racing a background spawn) never collide. An
/// 8-hex-character token is only 32 bits of entropy, which starts exhibiting non-negligible
/// birthday-collision probability well within the run-id volumes a long-lived multi-session cyrup
/// deployment could plausibly mint. This implementation instead derives the token from a random
/// (v4) UUID's 128 bits of entropy — collision probability at any volume this system could
/// plausibly reach is negligible — and renders it as the UUID's 32-hex-digit simple form (no
/// hyphens), which is still a compact, filesystem-safe, purely-hex token satisfying the letter of
/// both documents' "short hex token" shape while exceeding their stated entropy floor rather than
/// undershooting it.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RunId(std::sync::Arc<str>);

impl RunId {
    /// Mints a fresh [`RunId`] from 128 bits of random entropy. Never checks the filesystem or
    /// any other registry for pre-existence (R-SA-072) — the caller is responsible for creating
    /// the run directory via `mkdir` at spawn time, which is where an astronomically-unlikely
    /// collision would surface (as an `mkdir` failure), not here.
    #[must_use]
    pub fn new() -> Self {
        let token = uuid::Uuid::new_v4().as_simple().to_string();
        Self(std::sync::Arc::from(token.as_str()))
    }

    /// Wraps an already-known run-id token (e.g. one parsed back from a run directory name or a
    /// CLI argument) without minting new entropy. Does **not** validate the token's shape —
    /// safe-token validation (no path separators, no `..`, non-empty) against filesystem lookups
    /// is `background/control.rs`'s job (R-SA-087), not this constructor's; this is a plain,
    /// infallible wrap so callers that already trust their source (e.g. round-tripping a value
    /// this process itself minted and serialized) are not forced through a `Result`.
    #[must_use]
    pub fn from_token(token: impl Into<std::sync::Arc<str>>) -> Self {
        Self(token.into())
    }

    /// Borrows the token as a plain `&str` — the form used for directory-name construction,
    /// display, and safe-token validation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    /// Mints a fresh random [`RunId`], identical to [`RunId::new`]. Provided so `RunId` composes
    /// naturally with `#[derive(Default)]` call sites elsewhere in the crate; every `Default`
    /// call still mints fresh entropy, never a fixed/empty sentinel.
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RunId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use std::collections::HashSet;

    #[test]
    fn run_id_new_produces_a_purely_hex_token() {
        let id = RunId::new();
        assert!(
            id.as_str().chars().all(|c| c.is_ascii_hexdigit()),
            "token must be purely hex: {}",
            id.as_str()
        );
    }

    #[test]
    fn run_id_new_token_is_at_least_8_hex_chars() {
        // func-SA §4.5's illustrative floor ("8-hex-character random token"); this
        // implementation's 32-hex-digit UUIDv4 simple form comfortably exceeds it.
        let id = RunId::new();
        assert!(
            id.as_str().len() >= 8,
            "token too short: {} chars",
            id.as_str().len()
        );
    }

    #[test]
    fn run_id_new_has_no_path_separators_or_dots() {
        // Directory-name safety: a RunId is used verbatim as a path component (RunDir::new).
        let id = RunId::new();
        assert!(!id.as_str().contains('/'));
        assert!(!id.as_str().contains('\\'));
        assert!(!id.as_str().contains(".."));
        assert!(!id.as_str().is_empty());
    }

    #[test]
    fn run_id_new_is_unique_across_many_mintings() {
        // No pre-flight uniqueness check is performed (R-SA-072) — this test instead verifies
        // that the entropy source itself makes collisions practically unobservable across a
        // large sample, which is the property that makes skipping a uniqueness check safe.
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            let id = RunId::new();
            assert!(
                seen.insert(id.clone()),
                "duplicate RunId minted: {}",
                id.as_str()
            );
        }
    }

    #[test]
    fn run_id_display_matches_as_str() {
        let id = RunId::new();
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn run_id_from_token_round_trips_via_serde() {
        let id = RunId::from_token("deadbeefcafef00d");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(
            json, "\"deadbeefcafef00d\"",
            "serde(transparent) as bare string"
        );
        let back: RunId = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, id);
    }

    #[test]
    fn run_id_default_mints_fresh_entropy_each_time() {
        let a = RunId::default();
        let b = RunId::default();
        assert_ne!(a, b, "Default must not be a fixed/empty sentinel");
    }
}
