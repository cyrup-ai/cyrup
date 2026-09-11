//! [`SessionId`] — the launching orchestrator session's identity.
//!
//! Ports pi's `nonEmptyString(data.sessionId)` discipline (`result-files.ts:39-42`) and the
//! `typeof data.sessionId !== "string" || !data.sessionId` gate at `result-watcher.ts:408`.

use std::sync::Arc;

/// A non-empty session identifier.
///
/// # Why a newtype
///
/// Before this type the crate carried the session as `Option<String>` and re-derived "is it
/// really present?" at every use site — `executor/session_state.rs:45-49`,
/// `runner_main/entry.rs:242-247` and `run_status.rs:647-648` each applied their own
/// `.filter(|id| !id.is_empty())`. Three independent filters over one invariant is three chances
/// for them to drift, and an empty-string session id is indistinguishable from an absent one to
/// every consumer that forgets the filter. Parsing once, here, makes `Option<SessionId>` mean
/// exactly what it says.
///
/// It is also the reason [`crate::identity::CompletionOwnerId`] is a *separate* type rather than a
/// second `String`: `owns(session, owner)` takes both, and two `String` parameters can be
/// transposed silently at a call site while two distinct newtypes cannot.
///
/// # Not a `cyrup-session` dependency
///
/// This is an opaque identifier — a string this crate stores, compares and encodes into a path
/// segment. It carries no behaviour from and creates no linkage to `cyrup-session`. Doc comments
/// in `background/watch/` previously cited an architecture rule about `watch.rs` "having no
/// business knowing what a session is" as grounds for not gating delivery on session identity;
/// that reasoning never applied to an opaque token, and gating on it is what makes concurrent
/// cyrup instances in one directory correct.
///
/// # Emptiness, not whitespace
///
/// pi's `nonEmptyString` (`result-files.ts:39-42`) is a plain truthiness check — `""` is falsy,
/// `"  "` is not. [`SessionId::parse`] matches that exactly and does **not** trim. A session id is
/// frequently a full `.jsonl` path, and trimming would silently rewrite an identifier that must
/// round-trip byte-for-byte through the on-disk index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct SessionId(Arc<str>);

impl SessionId {
    /// The only fallible constructor. `None` for the empty string; every other value is accepted
    /// verbatim (no trimming — see the type docs).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            None
        } else {
            Some(Self(Arc::from(raw)))
        }
    }

    /// [`SessionId::parse`] over an `Option`, for threading through the many call sites that hold
    /// an `Option<String>` today. `Some("")` collapses to `None`, which is pi's own behaviour
    /// (`nonEmptyString` maps a falsy string to `undefined`).
    #[must_use]
    pub fn parse_opt(raw: Option<&str>) -> Option<Self> {
        raw.and_then(Self::parse)
    }

    /// Borrows the identifier for comparison, display, and encoding into an
    /// [`crate::identity::IndexSegment`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes **through** [`SessionId::parse`].
///
/// Deliberately not `#[serde(transparent)]` on the read side: a `ResultFile` or index entry is
/// read from a file on disk that this process may not have written, so deserialization is exactly
/// where an empty session id would otherwise slip past the invariant. An empty string is a hard
/// error rather than a silent `SessionId("")`, because a payload claiming an unusable identity is
/// malformed, not merely unattributed — an *absent* `sessionId` is represented by
/// `Option<SessionId>` being `None`, which is a different and legitimate state.
impl<'de> serde::Deserialize<'de> for SessionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("session id must not be empty"))
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

    #[test]
    fn parse_rejects_only_the_empty_string() {
        assert!(SessionId::parse("").is_none());
        assert_eq!(
            SessionId::parse("s1").map(|s| s.as_str().to_string()),
            Some("s1".to_string())
        );
    }

    #[test]
    fn parse_does_not_trim_whitespace() {
        // pi's `nonEmptyString` is a truthiness check; "  " is truthy. Trimming would rewrite an
        // identifier that must round-trip byte-for-byte through the on-disk index.
        let parsed = SessionId::parse("  ").expect("whitespace is a non-empty session id");
        assert_eq!(parsed.as_str(), "  ");
    }

    #[test]
    fn parse_accepts_a_full_jsonl_path_verbatim() {
        // The common real shape: pi session ids are frequently the session transcript's full path.
        let raw = "/home/u/.cyrup/agent/sessions/2026-09-06T01-02-03.jsonl";
        let parsed = SessionId::parse(raw).expect("a path is a valid session id");
        assert_eq!(parsed.as_str(), raw);
    }

    #[test]
    fn parse_opt_collapses_some_empty_to_none() {
        assert!(SessionId::parse_opt(None).is_none());
        assert!(SessionId::parse_opt(Some("")).is_none());
        assert!(SessionId::parse_opt(Some("s")).is_some());
    }

    #[test]
    fn deserialize_goes_through_parse_and_rejects_empty() {
        let ok: SessionId = serde_json::from_str("\"s1\"").expect("non-empty deserializes");
        assert_eq!(ok.as_str(), "s1");
        assert!(
            serde_json::from_str::<SessionId>("\"\"").is_err(),
            "an empty session id on disk must not deserialize into a SessionId"
        );
    }

    #[test]
    fn serialize_round_trips_transparently() {
        let id = SessionId::parse("sess-a").expect("non-empty");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, "\"sess-a\"");
        let back: SessionId = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, id);
    }

    #[test]
    fn optional_field_round_trips_absent_and_present() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Holder {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            session_id: Option<SessionId>,
        }
        let absent: Holder = serde_json::from_str("{}").expect("absent is legal");
        assert_eq!(absent.session_id, None);
        assert_eq!(serde_json::to_string(&absent).expect("ser"), "{}");

        let present: Holder =
            serde_json::from_str("{\"session_id\":\"s\"}").expect("present is legal");
        assert_eq!(
            present.session_id.as_ref().map(SessionId::as_str),
            Some("s")
        );
    }
}
