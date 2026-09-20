//! The capacity thresholds, the two value resolvers, the config-boundary validator, and the
//! resolved [`CapacityOptions`] every entry point in this module takes.
//!
//! Ports pi `active-async-capacity.ts:11-13` (the constants), `:80-83`
//! (`resolveMaxActiveAsyncRunsPerSession`), `:85-89` (`resolveAbandonedSlotReleaseAfterMs`) and
//! `extension/config.ts:86-98` (`validateCapacityConfig`), all @`v0.66.0`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::background::RunId;
use crate::background::reconcile::Liveness;
use crate::background::session_lease::ProcessStartIdentity;
use crate::registration::{AbandonedSlotRelease, CapacityConfig};

/// pi `DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS` (`:11`) — 20 minutes.
pub const DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 20 * 60 * 1000;

/// pi `MIN_ABANDONED_SLOT_RELEASE_AFTER_MS` (`:12`) — 5 minutes.
///
/// A **validation** bound, never a clamp: see [`validate_capacity_config`].
pub const MIN_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 5 * 60 * 1000;

/// pi `MAX_ABANDONED_SLOT_RELEASE_AFTER_MS` (`:13`) — 24 hours. A validation bound, not a clamp.
pub const MAX_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// pi `resolveMaxActiveAsyncRunsPerSession` (`:80-83`).
///
/// `0` means **unlimited**, not "refuse everything": upstream maps a non-integer, a negative AND
/// `0` to `undefined`, and `undefined` is what makes `acquireActiveAsyncCapacity` return
/// `undefined` without touching the filesystem (`:456`). cyrup's config field is already a
/// `u32`, so the "non-integer or negative" arms are unrepresentable and only the `0` arm survives
/// as a runtime decision.
#[must_use]
pub fn resolve_max_active_async_runs_per_session(value: Option<u32>) -> Option<u32> {
    value.filter(|limit| *limit != 0)
}

/// pi `resolveAbandonedSlotReleaseAfterMs` (`:85-89`).
///
/// # There is NO clamping here, and adding some would be a fail-OPEN divergence
///
/// Read upstream in full: `value === false` returns `false`; anything that is not a positive
/// integer returns the DEFAULT; everything else is returned **verbatim** — no `Math.min`, no
/// `Math.max`. [`MIN_ABANDONED_SLOT_RELEASE_AFTER_MS`]/[`MAX_ABANDONED_SLOT_RELEASE_AFTER_MS`] are
/// referenced in exactly one place upstream, [`validate_capacity_config`]'s original
/// (`extension/config.ts:86-98`), which **throws** on an out-of-range value.
///
/// So the real policy is *reject at the config boundary, substitute the default only for garbage*.
/// A clamping port would silently accept `1` and run a five-minute reclamation policy where
/// upstream refuses to start at all — the opposite of fail-closed, and on the one ladder that can
/// take a slot away from a run that is merely slow.
#[must_use]
pub fn resolve_abandoned_slot_release(value: Option<AbandonedSlotRelease>) -> AbandonedSlotRelease {
    match value {
        Some(AbandonedSlotRelease::Never) => AbandonedSlotRelease::Never,
        // pi `!Number.isInteger(value) || value < 1` (`:88`). An `i64` is always an integer, so
        // only the `< 1` arm survives; both it and the absent key fall to the default.
        Some(AbandonedSlotRelease::After(ms)) if ms >= 1 => AbandonedSlotRelease::After(ms),
        _ => AbandonedSlotRelease::After(DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS),
    }
}

/// pi `validateCapacityConfig` (`extension/config.ts:86-98` @`v0.66.0`) — the CONFIG-layer guard
/// that runs before any capacity decision is made with the value.
///
/// Bounds are **inclusive** on both ends (`:94-95` reject only `< MIN` and `> MAX`), and the
/// literal `false` is always accepted. `None` (key absent) is accepted, exactly as upstream's
/// `if (value === undefined) return`.
///
/// # Errors
///
/// Upstream's message verbatim (`:96`), including its lack of a trailing period — the same
/// per-layer asymmetry [`crate::exec::model_exclusions::validate_model_exclusions_config`]
/// documents, so an operator searching for the exact sentence they saw lands on the right layer:
///
/// `config.capacity.abandonedSlotReleaseAfterMs must be false or an integer from 300000 to 86400000`
pub fn validate_capacity_config(config: Option<&CapacityConfig>) -> Result<(), String> {
    let Some(value) = config.and_then(|config| config.abandoned_slot_release_after_ms) else {
        return Ok(());
    };
    let AbandonedSlotRelease::After(ms) = value else {
        // The literal `false` is always in range — it disables the ladder rather than setting it.
        return Ok(());
    };
    // Upstream's `value < MIN || value > MAX` (`:94-95`), spelled as the inclusive range clippy
    // asks for — both bounds are accepted, which is the property the test at the bounds pins.
    if !(MIN_ABANDONED_SLOT_RELEASE_AFTER_MS..=MAX_ABANDONED_SLOT_RELEASE_AFTER_MS).contains(&ms) {
        return Err(format!(
            "config.capacity.abandonedSlotReleaseAfterMs must be false or an integer from \
             {MIN_ABANDONED_SLOT_RELEASE_AFTER_MS} to {MAX_ABANDONED_SLOT_RELEASE_AFTER_MS}"
        ));
    }
    Ok(())
}

/// The fully-resolved runtime inputs every entry point of this module takes — pi's
/// `CapacityOptions` (`active-async-capacity.ts:40-48`) plus its
/// `{ liveWorkflowRunIds }` extension, with the ambient clock and liveness probe injected rather
/// than called internally.
///
/// # Why the two probes are injected
///
/// The same reason [`crate::background::reconcile`]'s own module doc gives: "simulated via a fake
/// reconciliation clock rather than actually waiting 24h in CI". The abandoned-slot ladder's whole
/// behaviour is an age comparison against a threshold measured in tens of minutes, and its release
/// decision turns on a pid probe whose two non-`Dead` answers must be provably distinguishable.
/// Neither is testable against a real clock or a real process table.
///
/// # Why the root is a resolved `PathBuf` and not a `&Roots`
///
/// A [`crate::background::active_async_capacity::ActiveAsyncCapacityHandle`] outlives the call
/// that created it — the spawn path holds it across the `runner-config.json` write and the
/// detached spawn — so options carrying a borrow would put a lifetime on the handle for no gain.
/// [`crate::background::active_async_capacity_root_in`] is the one production constructor.
#[derive(Clone)]
pub struct CapacityOptions {
    root_dir: PathBuf,
    live_workflow_run_ids: HashSet<RunId>,
    abandoned_slot_release: AbandonedSlotRelease,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pid_liveness: Arc<dyn Fn(u32) -> Liveness + Send + Sync>,
    pid_start_identity: Arc<dyn Fn(u32) -> Option<ProcessStartIdentity> + Send + Sync>,
}

impl std::fmt::Debug for CapacityOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapacityOptions")
            .field("root_dir", &self.root_dir)
            .field("live_workflow_run_ids", &self.live_workflow_run_ids.len())
            .field("abandoned_slot_release", &self.abandoned_slot_release)
            .finish_non_exhaustive()
    }
}

impl CapacityOptions {
    /// Options against a capacity root, with the production clock
    /// ([`crate::time::now_epoch_millis`]), the production liveness probe
    /// ([`crate::background::reconcile::check_pid_liveness`]), an empty live-workflow set and the
    /// default abandoned-slot policy.
    #[must_use]
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            live_workflow_run_ids: HashSet::new(),
            abandoned_slot_release: AbandonedSlotRelease::After(
                DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS,
            ),
            now: Arc::new(crate::time::now_epoch_millis),
            pid_liveness: Arc::new(crate::background::reconcile::check_pid_liveness),
            pid_start_identity: Arc::new(crate::background::session_lease::process_start_identity),
        }
    }

    /// pi `new Set(state.workflowControllers?.keys() ?? [])` at every call site.
    ///
    /// A live workflow's slot is NEVER reclaimed
    /// ([`super::inspect::workflow_release_verdict`]'s `:271` rung), so a leaked registry entry
    /// withholds capacity forever and a missing one reclaims a live run's slot. The set arrives
    /// OWNED (`live_workflow_run_ids` returns a `HashSet`) precisely so the executor's
    /// `std::sync::Mutex` is released before any `.await` in this module.
    #[must_use]
    pub fn with_live_workflow_run_ids(mut self, live: HashSet<RunId>) -> Self {
        self.live_workflow_run_ids = live;
        self
    }

    /// The ALREADY-RESOLVED policy — pass [`resolve_abandoned_slot_release`]'s output, never the
    /// raw config value, so the "absent means default" rung is applied exactly once.
    #[must_use]
    pub fn with_abandoned_slot_release(mut self, policy: AbandonedSlotRelease) -> Self {
        self.abandoned_slot_release = policy;
        self
    }

    /// Overrides the epoch-millisecond clock (pi `options.now`, `:42`).
    #[must_use]
    pub fn with_now(mut self, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now = now;
        self
    }

    /// Overrides the pid-liveness probe (pi `options.pidLiveness`, `:45`).
    #[must_use]
    pub fn with_pid_liveness(mut self, probe: Arc<dyn Fn(u32) -> Liveness + Send + Sync>) -> Self {
        self.pid_liveness = probe;
        self
    }

    /// Overrides the start-identity probe the no-proof fallback ladder pairs with
    /// [`Self::with_pid_liveness`].
    ///
    /// The two travel together and must be overridden together: the ladder is
    /// [`crate::background::reconcile::check_pid_identity_with`], which upgrades an `Alive` answer
    /// to `Dead` only when THIS probe reports an identity different from the one the owner record
    /// carries. A test that injects a liveness constant and leaves this one at the real `/proc`
    /// reader would be asking the kernel about a pid its fake liveness invented.
    #[must_use]
    pub fn with_pid_start_identity(
        mut self,
        probe: Arc<dyn Fn(u32) -> Option<ProcessStartIdentity> + Send + Sync>,
    ) -> Self {
        self.pid_start_identity = probe;
        self
    }

    /// The capacity root every pool hangs off — pi `options.rootDir ?? ACTIVE_ASYNC_CAPACITY_DIR`.
    #[must_use]
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// The live-workflow set this call must not reclaim against.
    #[must_use]
    pub fn live_workflow_run_ids(&self) -> &HashSet<RunId> {
        &self.live_workflow_run_ids
    }

    /// The resolved abandoned-slot policy.
    #[must_use]
    pub fn abandoned_slot_release(&self) -> AbandonedSlotRelease {
        self.abandoned_slot_release
    }

    /// "Now", in epoch milliseconds.
    #[must_use]
    pub fn now(&self) -> i64 {
        (self.now)()
    }

    /// The start identity `pid` is running under RIGHT NOW, for the recycled-pid rung.
    #[must_use]
    pub fn pid_start_identity(&self, pid: u32) -> Option<ProcessStartIdentity> {
        (self.pid_start_identity)(pid)
    }

    /// Classifies `pid` — **[`Liveness::Unknown`] is never death**, see
    /// [`super::inspect::abandoned_runner_release_verdict`].
    #[must_use]
    pub fn pid_liveness(&self, pid: u32) -> Liveness {
        (self.pid_liveness)(pid)
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
    fn no_configured_cap_means_unlimited_for_both_absent_and_zero() {
        // pi `:80-83` — `value === 0 ? undefined : value`. Absent and `0` are the SAME answer.
        assert_eq!(resolve_max_active_async_runs_per_session(None), None);
        assert_eq!(resolve_max_active_async_runs_per_session(Some(0)), None);
        assert_eq!(resolve_max_active_async_runs_per_session(Some(1)), Some(1));
        assert_eq!(
            resolve_max_active_async_runs_per_session(Some(64)),
            Some(64)
        );
    }

    #[test]
    fn release_after_ms_below_the_minimum_is_refused_by_config_validation() {
        // §D1: the bound is a REJECTION, not a clamp. Both halves matter — a clamping
        // implementation would pass the first assertion and fail the second.
        let config = CapacityConfig {
            abandoned_slot_release_after_ms: Some(AbandonedSlotRelease::After(
                MIN_ABANDONED_SLOT_RELEASE_AFTER_MS - 1,
            )),
        };
        let error = validate_capacity_config(Some(&config)).expect_err("out of range");
        assert_eq!(
            error,
            "config.capacity.abandonedSlotReleaseAfterMs must be false or an integer from 300000 to 86400000"
        );
        assert_eq!(
            resolve_abandoned_slot_release(config.abandoned_slot_release_after_ms),
            AbandonedSlotRelease::After(MIN_ABANDONED_SLOT_RELEASE_AFTER_MS - 1),
            "the resolver returns the value VERBATIM — upstream has no clamp"
        );
    }

    #[test]
    fn release_after_ms_above_the_maximum_is_refused_by_config_validation() {
        let config = CapacityConfig {
            abandoned_slot_release_after_ms: Some(AbandonedSlotRelease::After(
                MAX_ABANDONED_SLOT_RELEASE_AFTER_MS + 1,
            )),
        };
        assert!(validate_capacity_config(Some(&config)).is_err());
        assert_eq!(
            resolve_abandoned_slot_release(config.abandoned_slot_release_after_ms),
            AbandonedSlotRelease::After(MAX_ABANDONED_SLOT_RELEASE_AFTER_MS + 1),
        );
    }

    #[test]
    fn release_after_ms_at_the_minimum_and_maximum_are_accepted() {
        // pi `:94-95` rejects only `< MIN` and `> MAX`: both bounds are inclusive.
        for ms in [
            MIN_ABANDONED_SLOT_RELEASE_AFTER_MS,
            MAX_ABANDONED_SLOT_RELEASE_AFTER_MS,
        ] {
            let config = CapacityConfig {
                abandoned_slot_release_after_ms: Some(AbandonedSlotRelease::After(ms)),
            };
            assert!(
                validate_capacity_config(Some(&config)).is_ok(),
                "{ms} is in range"
            );
        }
        // `false` and an absent object are always accepted.
        assert!(validate_capacity_config(None).is_ok());
        assert!(
            validate_capacity_config(Some(&CapacityConfig {
                abandoned_slot_release_after_ms: Some(AbandonedSlotRelease::Never),
            }))
            .is_ok()
        );
        assert!(
            validate_capacity_config(Some(&CapacityConfig {
                abandoned_slot_release_after_ms: None,
            }))
            .is_ok()
        );
    }

    #[test]
    fn the_three_states_resolve_to_three_different_policies() {
        assert_eq!(
            resolve_abandoned_slot_release(None),
            AbandonedSlotRelease::After(DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS),
        );
        assert_eq!(
            resolve_abandoned_slot_release(Some(AbandonedSlotRelease::Never)),
            AbandonedSlotRelease::Never,
        );
        assert_eq!(
            resolve_abandoned_slot_release(Some(AbandonedSlotRelease::After(900_000))),
            AbandonedSlotRelease::After(900_000),
        );
        // pi `:88`'s `value < 1` rung: garbage falls to the DEFAULT, never to `Never`.
        assert_eq!(
            resolve_abandoned_slot_release(Some(AbandonedSlotRelease::After(0))),
            AbandonedSlotRelease::After(DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS),
        );
        assert_eq!(
            resolve_abandoned_slot_release(Some(AbandonedSlotRelease::After(-1))),
            AbandonedSlotRelease::After(DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS),
        );
    }

    #[test]
    fn the_capacity_value_round_trips_json_false_and_a_bare_number() {
        // The three-state distinction is an ON-DISK format: `false` must survive as `false`, never
        // as `null`, `0` or an object.
        let never: CapacityConfig =
            serde_json::from_str(r#"{"abandonedSlotReleaseAfterMs":false}"#).expect("parses");
        assert_eq!(
            never.abandoned_slot_release_after_ms,
            Some(AbandonedSlotRelease::Never)
        );
        assert_eq!(
            serde_json::to_string(&never).expect("ser"),
            r#"{"abandonedSlotReleaseAfterMs":false}"#
        );

        let after: CapacityConfig =
            serde_json::from_str(r#"{"abandonedSlotReleaseAfterMs":900000}"#).expect("parses");
        assert_eq!(
            after.abandoned_slot_release_after_ms,
            Some(AbandonedSlotRelease::After(900_000))
        );
        assert_eq!(
            serde_json::to_string(&after).expect("ser"),
            r#"{"abandonedSlotReleaseAfterMs":900000}"#
        );

        let unset: CapacityConfig = serde_json::from_str("{}").expect("parses");
        assert_eq!(unset.abandoned_slot_release_after_ms, None);
        assert_eq!(serde_json::to_string(&unset).expect("ser"), "{}");

        // `true` is upstream's "not a number" arm and is refused, never read as "enabled".
        assert!(
            serde_json::from_str::<CapacityConfig>(r#"{"abandonedSlotReleaseAfterMs":true}"#)
                .is_err()
        );
    }
}
