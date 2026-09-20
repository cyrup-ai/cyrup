//! [`RunnerProcessInstanceId`] — the identity the whole process-terminal proof is keyed on.
//!
//! Split out of `background/process_terminal/mod.rs` behind that module's facade, exactly as
//! [`RunId`](crate::background::RunId) is split out of `background/mod.rs`.

/// pi `runnerProcessInstanceId` — the opaque token minted ONCE per launch, in the ORCHESTRATOR,
/// immediately before the runner config is written, and carried into the runner through that
/// config (`runs/background/async-execution.ts:707` `const runnerProcessInstanceId =
/// randomUUID();`, spread into `launchConfig` at `:710`, @v0.68.0).
///
/// Every consumer of the process-terminal artifact MATCHES on this value:
/// [`validate_proof`](super::validate_proof) refuses a proof whose id differs from the one the
/// reader expected (`process-terminal.ts:166`), and the capacity release rung accepts a slot
/// release only on an `observed` proof carrying the owner's own id
/// (`active-async-capacity.ts:227-235`). A token that two different runners could share would
/// make both of those checks vacuous.
///
/// # Why a fresh v4 uuid and not the `(runner_pid, runner_started_at)` pair cyrup already records
///
/// [`ActiveAsyncCapacityOwner::runner_pid`](crate::background::active_async_capacity::ActiveAsyncCapacityOwner::runner_pid)
/// plus its `runner_started_at` sibling is what `active_async_capacity` fell back to while this
/// identity did not exist, and it is still the no-proof fallback ladder beneath the proof rung. It
/// cannot BECOME the identity: an OS pid is reusable, and
/// [`check_pid_liveness`](crate::background::reconcile::check_pid_liveness) is a bare `kill(pid,
/// 0)`, so a recycled pid reads alive. Keying the proof on a reusable value re-opens the very
/// pid-reuse hole the proof exists to close, one layer up — a recycled pid landing on the same
/// `runner_started_at` millisecond is a matchable collision. 128 bits of random entropy cannot
/// collide at any volume this system can reach. (The fallback ladder closes its own half of that
/// hole differently, with
/// [`check_pid_identity_with`](crate::background::reconcile::check_pid_identity_with)'s start-identity
/// rung — evidence about a pid, where this is an identity that needs none.)
///
/// The mint is [`RunId::new`](crate::background::RunId::new)'s, verbatim: a v4 uuid rendered in
/// its 32-hex-digit simple form. `uuid` is already a direct dependency of this crate, so the
/// decision costs nothing.
///
/// On the wire it is a plain string (`#[serde(transparent)]`), which is upstream's
/// `runnerProcessInstanceId: string` unchanged.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RunnerProcessInstanceId(std::sync::Arc<str>);

impl RunnerProcessInstanceId {
    /// Mints a fresh instance id from 128 bits of random entropy — pi's `randomUUID()`
    /// (`async-execution.ts:707`).
    #[must_use]
    pub fn new() -> Self {
        let token = uuid::Uuid::new_v4().as_simple().to_string();
        Self(std::sync::Arc::from(token.as_str()))
    }

    /// Wraps an already-known token (one read back from a runner config, a candidate, a proof or a
    /// capacity owner) without minting new entropy. Deliberately infallible and non-validating,
    /// mirroring [`RunId::from_token`](crate::background::RunId::from_token): the token's only
    /// contract is that two runners never share one, which is the MINT's job, not the wrap's.
    #[must_use]
    pub fn from_token(token: impl Into<std::sync::Arc<str>>) -> Self {
        Self(token.into())
    }

    /// Borrows the token as a plain `&str` — the form every match, render and JSON write uses.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunnerProcessInstanceId {
    /// Mints fresh entropy, identical to [`RunnerProcessInstanceId::new`] — never a fixed
    /// sentinel, for [`RunId::default`](crate::background::RunId::default)'s own reason.
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunnerProcessInstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RunnerProcessInstanceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::collections::HashSet;

    /// The mint is the whole reason `validate_proof`'s two identity checks mean anything: two
    /// runners must never be able to present the same id.
    #[test]
    fn a_minted_instance_id_is_unique_across_many_mintings() {
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            assert!(
                seen.insert(RunnerProcessInstanceId::new()),
                "duplicate RunnerProcessInstanceId minted"
            );
        }
    }

    /// On-disk shape: a bare JSON string, which is pi's `runnerProcessInstanceId: string`.
    #[test]
    fn an_instance_id_serializes_as_a_bare_string() {
        let id = RunnerProcessInstanceId::from_token("cafef00ddeadbeef");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, "\"cafef00ddeadbeef\"");
        let back: RunnerProcessInstanceId = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, id);
        assert_eq!(back.to_string(), back.as_str());
    }

    /// Purely hex and filesystem/JSON-safe, like the `RunId` it is modelled on — the id is
    /// interpolated into operator-facing sentences and event lines.
    #[test]
    fn a_minted_instance_id_is_purely_hex() {
        let id = RunnerProcessInstanceId::new();
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id.as_str().len(), 32);
    }
}
