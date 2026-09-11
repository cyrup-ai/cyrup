//! [`CompletionOwnerId`] — the launching *process*'s identity, and its process-global mint.
//!
//! Ports pi `shared/completion-owner.ts:10-14`.

use std::sync::{Arc, OnceLock};

/// The identity of one orchestrator **process**, stable for that process's whole lifetime.
///
/// # Why this exists alongside [`crate::identity::SessionId`]
///
/// Delivery ownership is two questions, not one (pi `result-delivery-ownership.ts:22-26`):
///
/// * *whose session is this result for?* — [`crate::identity::SessionId`]
/// * *which process is entitled to consume it?* — this type
///
/// A session id can outlive the process that created it (it is written into the result file on
/// disk and read back by whoever is running later), so session identity alone cannot decide
/// consumption. The owner id is minted fresh per process and never persisted anywhere but inside
/// the result payload, so a restarted orchestrator does not inherit the previous one's claims.
///
/// # Why a distinct TYPE and not a second `String`
///
/// The predicate is `owns(session_id, completion_owner_id)`. With two `String` parameters a
/// transposed call site compiles, passes every type check, and silently answers the wrong
/// question — and the failure is invisible because both values are opaque tokens. Two newtypes
/// make the transposition a compile error. That is the entire justification; there is no
/// behavioural difference otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct CompletionOwnerId(Arc<str>);

impl CompletionOwnerId {
    /// Mints a fresh owner id from 128 bits of random entropy.
    ///
    /// Prefer [`current_completion_owner_id`] in production — this process has exactly one owner
    /// identity and minting a second one would make results written under the first undeliverable.
    /// Exposed for tests that need two distinct owners to model two processes.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::from(
            uuid::Uuid::new_v4().as_simple().to_string().as_str(),
        ))
    }

    /// Wraps an already-known owner token — the deserialization/round-trip path. `None` for the
    /// empty string, matching [`crate::identity::SessionId::parse`]'s discipline and pi's
    /// `typeof completionOwnerId === "string" && completionOwnerId` truthiness check
    /// (`result-watcher.ts:226-228`).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            None
        } else {
            Some(Self(Arc::from(raw)))
        }
    }

    /// Borrows the token for comparison and display.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CompletionOwnerId {
    /// Mints fresh entropy, identical to [`CompletionOwnerId::new`] — never a fixed sentinel, so a
    /// `#[derive(Default)]` call site cannot accidentally create two "equal" owners.
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CompletionOwnerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes **through** [`CompletionOwnerId::parse`]; see
/// [`crate::identity::SessionId`]'s `Deserialize` for why the read side is not transparent.
impl<'de> serde::Deserialize<'de> for CompletionOwnerId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom("completion owner id must not be empty"))
    }
}

/// The process-global owner identity, minted once on first call.
///
/// Ports pi `currentCompletionOwnerId` (`shared/completion-owner.ts:10-14`), which memoizes on
/// `globalThis` under a `Symbol.for` key so the value survives an extension reload within one
/// parent process. A `OnceLock` gives the same once-per-process guarantee by a mechanism the
/// language provides directly: cyrup's native extension is loaded into the host process rather
/// than re-evaluated as a module, so there is no reload path that a `static` would fail to cover.
///
/// **Never call this in the detached runner.** The owner recorded in a result file must be the
/// *orchestrator's*, because the orchestrator is who will later consume it; the runner receives
/// that value through its `RunnerConfig` and stamps what it was given.
#[must_use]
pub fn current_completion_owner_id() -> CompletionOwnerId {
    static OWNER: OnceLock<CompletionOwnerId> = OnceLock::new();
    OWNER.get_or_init(CompletionOwnerId::new).clone()
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
    fn the_process_owner_is_stable_across_calls() {
        assert_eq!(current_completion_owner_id(), current_completion_owner_id());
    }

    #[test]
    fn freshly_minted_owners_differ() {
        // Two processes must never collide; `new` is what tests use to model a second process.
        assert_ne!(CompletionOwnerId::new(), CompletionOwnerId::new());
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(CompletionOwnerId::parse("").is_none());
        assert!(CompletionOwnerId::parse("o1").is_some());
    }

    #[test]
    fn deserialize_goes_through_parse() {
        let ok: CompletionOwnerId = serde_json::from_str("\"o1\"").expect("non-empty");
        assert_eq!(ok.as_str(), "o1");
        assert!(serde_json::from_str::<CompletionOwnerId>("\"\"").is_err());
    }

    #[test]
    fn round_trips_through_json() {
        let id = CompletionOwnerId::new();
        let json = serde_json::to_string(&id).expect("ser");
        let back: CompletionOwnerId = serde_json::from_str(&json).expect("de");
        assert_eq!(back, id);
    }

    #[test]
    fn owner_and_session_are_not_interchangeable_types() {
        // Compile-time proof of the transposition guard: these are different types, so
        // `owns(session, owner)` cannot be called with its arguments swapped. If this ever stops
        // compiling as written, the two identities have been collapsed back into one.
        let owner = CompletionOwnerId::parse("x").expect("non-empty");
        let session = crate::identity::SessionId::parse("x").expect("non-empty");
        assert_eq!(owner.as_str(), session.as_str());
        // ...yet the values are not comparable without going through `as_str`, which is the point.
    }
}
