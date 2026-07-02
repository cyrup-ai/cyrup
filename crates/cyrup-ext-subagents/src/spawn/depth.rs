//! Canonical depth-propagation logic (R-SA-054, R-SA-055, R-SA-056 — the single canonical
//! statement of the recursion guard; every other reference to depth in func-SA/arch-SA,
//! including §5.1's R-SA-022 and §5.6's R-SA-009, defers to the algorithm in this module rather
//! than restating it).
//!
//! The recursion guard is enforced purely via two OS environment variables read by the child
//! process at startup — `CYRUP_SUBAGENT_DEPTH` (current recursion depth) and
//! `CYRUP_SUBAGENT_MAX_DEPTH` (the effective ceiling inherited from the parent). There is no
//! external counter file, database, or IPC round-trip: depth propagates transitively through OS
//! environment inheritance alone (R-SA-054). Every function in this module is pure — the only
//! I/O performed anywhere in this file is `std::env::var` reads in [`resolve_effective_depth`].
//!
//! Callers MUST run [`resolve_effective_depth`] and check [`is_blocked`] before any spawn,
//! discovery, or worktree setup is attempted (R-SA-055) — a blocked check must short-circuit
//! into an error result telling the caller to complete the task directly, rather than proceeding
//! into any of that setup work.

use std::collections::HashMap;

/// The current recursion depth and the effective (already-tightened) max-depth ceiling in force
/// for the process that resolved it.
///
/// `current_depth` is how many subagent-spawn hops separate this process from the original
/// top-level invocation (the top-level process itself resolves `current_depth == 0`).
/// `max_depth` is the ceiling that `current_depth` is checked against by [`is_blocked`] — it is
/// itself the result of every ancestor's tightening-only merge (R-SA-056), never re-derived by
/// this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthEnvelope {
    /// Number of subagent-spawn hops from the top-level invocation to this process.
    pub current_depth: u32,
    /// The effective max-depth ceiling in force for this process (already tightened by every
    /// ancestor's own `max_subagent_depth`, per R-SA-056).
    pub max_depth: u32,
}

/// The env var carrying the current recursion depth (parse-or-default-`0`).
pub const DEPTH_ENV_VAR: &str = "CYRUP_SUBAGENT_DEPTH";

/// The env var carrying the effective max-depth ceiling (parse-or-default-from-config).
pub const MAX_DEPTH_ENV_VAR: &str = "CYRUP_SUBAGENT_MAX_DEPTH";

/// Reads `CYRUP_SUBAGENT_DEPTH` / `CYRUP_SUBAGENT_MAX_DEPTH` from the process environment at
/// startup, before any spawn, discovery, or worktree-setup logic runs (R-SA-055).
///
/// `CYRUP_SUBAGENT_DEPTH` parses as a `u32`, defaulting to `0` if absent or unparsable — the
/// top-level (non-subagent) invocation of the binary has no ancestor and is therefore always
/// depth `0`. `CYRUP_SUBAGENT_MAX_DEPTH` parses as a `u32`, defaulting to the caller-supplied
/// `config_max` (the locally configured `max_subagent_depth`, arch-SA §3.8) if absent or
/// unparsable — this is what lets a genuinely top-level process (with no inherited ceiling in
/// its environment at all) still get a sane, locally configured ceiling rather than an unbounded
/// one.
///
/// A malformed value for either variable (non-numeric, negative, out of `u32` range) is treated
/// identically to an absent one and falls back to the same default — this function never errors
/// and never panics.
#[must_use]
pub fn resolve_effective_depth(config_max: u32) -> DepthEnvelope {
    resolve_effective_depth_from(config_max, |key| std::env::var(key).ok())
}

/// The pure core of [`resolve_effective_depth`], parameterized over the env lookup so the
/// parse-or-default behavior can be exercised deterministically in unit tests without mutating
/// real process environment state (`std::env::set_var`/`remove_var` are `unsafe` as of the 2024
/// edition and process env is global mutable state shared across concurrently-run tests either
/// way — this crate is `#![forbid(unsafe_code)]`, so tests inject a lookup closure instead of
/// touching the real environment at all).
fn resolve_effective_depth_from(
    config_max: u32,
    lookup: impl Fn(&str) -> Option<String>,
) -> DepthEnvelope {
    let current_depth = lookup(DEPTH_ENV_VAR)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max_depth = lookup(MAX_DEPTH_ENV_VAR)
        .and_then(|v| v.parse().ok())
        .unwrap_or(config_max);
    DepthEnvelope {
        current_depth,
        max_depth,
    }
}

/// Returns `true` once `envelope.current_depth` has reached (or, defensively, exceeded)
/// `envelope.max_depth` — i.e. this process is not permitted to spawn any further nested
/// subagent (R-SA-055).
///
/// Callers MUST perform this check before any spawn, discovery, or worktree setup; on `true`
/// the caller must surface an error telling the invoker to complete the task directly rather
/// than attempting to delegate further (see [`crate::error::SubagentError::DepthExceeded`]).
#[must_use]
pub fn is_blocked(envelope: &DepthEnvelope) -> bool {
    envelope.current_depth >= envelope.max_depth
}

/// Computes the [`DepthEnvelope`] to propagate into a freshly spawned child, per R-SA-056 (the
/// canonical, sole statement of the tightening-only rule / DI-SA-4).
///
/// `next_depth = current.current_depth + 1`.
///
/// `next_max = min(current.max_depth, agent_max_subagent_depth)` when the spawning agent
/// declares its own `max_subagent_depth`; otherwise `next_max = current.max_depth` (the
/// inherited ceiling passes through unchanged). An agent's own configured `max_subagent_depth`
/// may only ever *tighten* (lower) the ceiling it hands to its own children — it can never raise
/// it above what this process itself inherited, even when the agent's resolved tools include
/// nested-fanout capability. A last-write-wins or `max()` implementation is non-conformant with
/// R-SA-056 and MUST NOT be introduced here or anywhere else in this crate.
#[must_use]
pub fn next_envelope(
    current: &DepthEnvelope,
    agent_max_subagent_depth: Option<u32>,
) -> DepthEnvelope {
    let next_max = agent_max_subagent_depth
        .map(|agent_max| current.max_depth.min(agent_max))
        .unwrap_or(current.max_depth);
    DepthEnvelope {
        current_depth: current.current_depth + 1,
        max_depth: next_max,
    }
}

/// Renders a [`DepthEnvelope`] as the two string env-var entries (`CYRUP_SUBAGENT_DEPTH`,
/// `CYRUP_SUBAGENT_MAX_DEPTH`) to be inserted into a child's spawn-env overlay map.
///
/// Per R-SA-048's ordering, this overlay MUST be inserted into the child's env map *after* the
/// inherited-env base map has been populated — since `tokio::process::Command::envs()` merges
/// on top of the unmodified inherited environment whenever `env_clear()` is never called, the
/// spawn boundary (`spawn/mod.rs`) achieves correct inherit-then-override semantics simply by
/// calling `.envs(overlay)` with this map layered on last; no special-cased merge logic is
/// needed here or at the call site.
#[must_use]
pub fn to_env_overlay(envelope: &DepthEnvelope) -> HashMap<String, String> {
    let mut overlay = HashMap::with_capacity(2);
    overlay.insert(
        DEPTH_ENV_VAR.to_string(),
        envelope.current_depth.to_string(),
    );
    overlay.insert(
        MAX_DEPTH_ENV_VAR.to_string(),
        envelope.max_depth.to_string(),
    );
    overlay
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use std::collections::HashMap;

    /// Builds a lookup closure over a fixed `HashMap`, standing in for `std::env::var` without
    /// touching real (global, mutable, `unsafe`-to-mutate-under-edition-2024) process
    /// environment state.
    fn lookup_from(vars: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
        move |key| vars.get(key).map(|v| (*v).to_string())
    }

    #[test]
    fn resolve_defaults_to_zero_depth_and_config_max_when_env_absent() {
        let envelope = resolve_effective_depth_from(2, lookup_from(HashMap::new()));

        assert_eq!(
            envelope.current_depth, 0,
            "absent depth var parses-or-defaults to 0"
        );
        assert_eq!(
            envelope.max_depth, 2,
            "absent max-depth var falls back to config_max"
        );
    }

    #[test]
    fn resolve_defaults_on_unparsable_values_rather_than_erroring() {
        let vars = HashMap::from([
            (DEPTH_ENV_VAR, "not-a-number"),
            (MAX_DEPTH_ENV_VAR, "also-not-a-number"),
        ]);

        let envelope = resolve_effective_depth_from(5, lookup_from(vars));

        assert_eq!(
            envelope.current_depth, 0,
            "unparsable depth falls back to 0, never errors"
        );
        assert_eq!(
            envelope.max_depth, 5,
            "unparsable max-depth falls back to config_max"
        );
    }

    #[test]
    fn resolve_reads_valid_inherited_values_from_env() {
        let vars = HashMap::from([(DEPTH_ENV_VAR, "3"), (MAX_DEPTH_ENV_VAR, "7")]);

        let envelope = resolve_effective_depth_from(2, lookup_from(vars));

        assert_eq!(
            envelope.current_depth, 3,
            "inherited depth is read verbatim"
        );
        assert_eq!(
            envelope.max_depth, 7,
            "inherited max-depth overrides config_max"
        );
    }

    #[test]
    fn resolve_treats_negative_depth_as_unparsable_and_defaults() {
        let vars = HashMap::from([(DEPTH_ENV_VAR, "-1")]);

        let envelope = resolve_effective_depth_from(4, lookup_from(vars));

        assert_eq!(
            envelope.current_depth, 0,
            "negative depth is not a valid u32 and defaults to 0"
        );
        assert_eq!(envelope.max_depth, 4);
    }

    #[test]
    fn resolve_effective_depth_reads_the_real_process_environment() {
        // A smoke test exercising the actual public entry point (which reads real
        // std::env::var) rather than the injectable core, so the wiring between
        // `resolve_effective_depth` and `resolve_effective_depth_from` is covered without this
        // crate ever mutating real (global, `unsafe`-to-mutate-under-edition-2024) process env.
        // We deliberately do not set either var here; if some outer harness happens to already
        // have CYRUP_SUBAGENT_MAX_DEPTH set, the resolved max_depth reflects that real value
        // rather than config_max — either way the invariant below must hold.
        let config_max = 9;
        let envelope = resolve_effective_depth(config_max);
        assert!(
            envelope.max_depth == config_max || std::env::var(MAX_DEPTH_ENV_VAR).is_ok(),
            "max_depth must equal config_max unless the real env var is actually set"
        );
    }

    #[test]
    fn is_blocked_false_while_strictly_below_ceiling() {
        let envelope = DepthEnvelope {
            current_depth: 1,
            max_depth: 2,
        };
        assert!(!is_blocked(&envelope));
    }

    #[test]
    fn is_blocked_true_at_ceiling() {
        let envelope = DepthEnvelope {
            current_depth: 2,
            max_depth: 2,
        };
        assert!(
            is_blocked(&envelope),
            "current == max must block, not just current > max"
        );
    }

    #[test]
    fn is_blocked_true_beyond_ceiling_defensively() {
        // Should never occur in practice (each hop only ever increments by 1 past a checked
        // gate), but the comparison must still be a safe `>=`, not an `==`, for defense in depth.
        let envelope = DepthEnvelope {
            current_depth: 5,
            max_depth: 2,
        };
        assert!(is_blocked(&envelope));
    }

    #[test]
    fn is_blocked_false_at_depth_zero_with_positive_ceiling() {
        let envelope = DepthEnvelope {
            current_depth: 0,
            max_depth: 1,
        };
        assert!(!is_blocked(&envelope));
    }

    #[test]
    fn next_envelope_increments_depth_by_exactly_one() {
        let current = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let next = next_envelope(&current, None);
        assert_eq!(next.current_depth, 1);
    }

    #[test]
    fn next_envelope_tightens_when_agent_declares_a_lower_ceiling() {
        // R-SA-056 / A-SA-6: inherited max 3, agent declares max 1 -> effective ceiling 1.
        let current = DepthEnvelope {
            current_depth: 0,
            max_depth: 3,
        };
        let next = next_envelope(&current, Some(1));
        assert_eq!(next.max_depth, 1, "agent's tighter declared max must win");
    }

    #[test]
    fn next_envelope_never_relaxes_when_agent_declares_a_higher_ceiling() {
        // The other direction of A-SA-6: an agent trying to RAISE its inherited ceiling must be
        // clamped back down to the inherited value — min(), never max() or last-write-wins.
        let current = DepthEnvelope {
            current_depth: 0,
            max_depth: 1,
        };
        let next = next_envelope(&current, Some(10));
        assert_eq!(
            next.max_depth, 1,
            "an agent-declared max ABOVE the inherited ceiling must never relax it"
        );
    }

    #[test]
    fn next_envelope_passes_through_inherited_max_when_agent_declares_none() {
        let current = DepthEnvelope {
            current_depth: 2,
            max_depth: 4,
        };
        let next = next_envelope(&current, None);
        assert_eq!(
            next.max_depth, 4,
            "no agent-declared max means the inherited ceiling is unchanged"
        );
    }

    #[test]
    fn next_envelope_tightens_when_agent_declares_the_same_ceiling() {
        let current = DepthEnvelope {
            current_depth: 0,
            max_depth: 2,
        };
        let next = next_envelope(&current, Some(2));
        assert_eq!(next.max_depth, 2);
    }

    #[test]
    fn next_envelope_handles_zero_declared_max_by_tightening_to_zero() {
        // An agent explicitly declaring maxSubagentDepth: 0 must produce a child envelope that
        // is immediately blocked for that child's own descendants.
        let current = DepthEnvelope {
            current_depth: 0,
            max_depth: 3,
        };
        let next = next_envelope(&current, Some(0));
        assert_eq!(next.max_depth, 0);
        assert!(is_blocked(&next));
    }

    #[test]
    fn to_env_overlay_renders_both_vars_as_decimal_strings() {
        let envelope = DepthEnvelope {
            current_depth: 2,
            max_depth: 5,
        };
        let overlay = to_env_overlay(&envelope);
        assert_eq!(overlay.get(DEPTH_ENV_VAR).map(String::as_str), Some("2"));
        assert_eq!(
            overlay.get(MAX_DEPTH_ENV_VAR).map(String::as_str),
            Some("5")
        );
        assert_eq!(overlay.len(), 2);
    }

    #[test]
    fn round_trip_resolve_then_render_is_stable() {
        let vars = HashMap::from([(DEPTH_ENV_VAR, "1"), (MAX_DEPTH_ENV_VAR, "3")]);

        let resolved = resolve_effective_depth_from(99, lookup_from(vars));
        let next = next_envelope(&resolved, Some(2));
        let overlay = to_env_overlay(&next);

        assert_eq!(overlay.get(DEPTH_ENV_VAR).map(String::as_str), Some("2"));
        assert_eq!(
            overlay.get(MAX_DEPTH_ENV_VAR).map(String::as_str),
            Some("2")
        );
    }
}
